//! The SQL functions a rule may call, built from the allowlist rather than subtracted from
//! `DataFusion`'s defaults (R4, KTD4).
//!
//! Every function is a `DaemonEye` implementation, so a built-in a future `DataFusion` release adds
//! is unreachable by omission instead of by someone remembering to remove it. [`allowlisted_udfs`]
//! returns exactly [`ALLOWED_SQL_FUNCTIONS`](crate::detection::ALLOWED_SQL_FUNCTIONS); a test pins
//! the two together.
//!
//! # Units
//!
//! `length` and `instr` both count **characters** (Unicode scalar values), not bytes, so
//! `instr(x, '/')` and `length(x)` count in the same unit, so a position from one is valid against the other on a non-ASCII value. This is the convention
//! of `SQLite`, `MySQL` and `PostgreSQL`; a byte offset would be meaningless to a rule author who
//! cannot see the encoding. Every function returns NULL for a NULL operand.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, ArrayRef, BinaryBuilder, BooleanArray, Int64Array, StringArray,
};
use datafusion::arrow::compute::{cast, like as arrow_like};
use datafusion::arrow::datatypes::DataType;
use datafusion::common::cast::{as_binary_array, as_string_array};
use datafusion::common::types::{logical_binary, logical_string};
use datafusion::common::{DataFusionError, Result};
use datafusion::logical_expr::{
    Coercion, ColumnarValue, ScalarFunctionArgs, ScalarUDF, ScalarUDFImpl, Signature,
    TypeSignatureClass, Volatility,
};

use crate::detection::execution::regexp::{LatencySink, RegexpUdf};
use crate::detection::regex_cache::RegexCache;

/// Names registered by [`allowlisted_udfs`], including the `match` alias, sorted.
pub const ALLOWLISTED_UDF_NAMES: &[&str] =
    &["hex", "instr", "length", "like", "match", "regexp", "unhex"];

type Kernel = fn(&[ArrayRef]) -> Result<ArrayRef>;

/// A stateless function over already-materialised argument arrays.
///
/// Identity is the name: two instances with one name are one function.
#[derive(Debug)]
struct ArrayFn {
    name: &'static str,
    signature: Signature,
    returns: DataType,
    kernel: Kernel,
}

impl PartialEq for ArrayFn {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for ArrayFn {}

impl Hash for ArrayFn {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl ScalarUDFImpl for ArrayFn {
    fn name(&self) -> &str {
        self.name
    }

    fn signature(&self) -> &Signature {
        &self.signature
    }

    fn return_type(&self, _arg_types: &[DataType]) -> Result<DataType> {
        Ok(self.returns.clone())
    }

    fn invoke_with_args(&self, args: ScalarFunctionArgs) -> Result<ColumnarValue> {
        let arrays = ColumnarValue::values_to_arrays(&args.args)?;
        (self.kernel)(&arrays).map(ColumnarValue::Array)
    }
}

/// A signature that accepts only string-typed arguments, with no implicit cast from numbers.
///
/// `Signature::exact` would coerce an `Int64` column to text, so `length(pid)` would plan and
/// return the digit count; a rule that does that has a bug, and the engine should say so.
pub(super) fn string_signature(arity: usize) -> Signature {
    let string = || Coercion::new_exact(TypeSignatureClass::Native(logical_string()));
    Signature::coercible(
        (0..arity).map(|_| string()).collect(),
        Volatility::Immutable,
    )
}

fn binary_signature() -> Signature {
    Signature::coercible(
        vec![Coercion::new_exact(TypeSignatureClass::Native(
            logical_binary(),
        ))],
        Volatility::Immutable,
    )
}

/// `array` as `Utf8`, whatever string encoding the plan delivered.
pub(super) fn as_utf8(array: &ArrayRef) -> Result<ArrayRef> {
    Ok(cast(array, &DataType::Utf8)?)
}

fn as_binary(array: &ArrayRef) -> Result<ArrayRef> {
    Ok(cast(array, &DataType::Binary)?)
}

fn udf(
    name: &'static str,
    signature: Signature,
    returns: DataType,
    kernel: Kernel,
) -> Arc<ScalarUDF> {
    Arc::new(ScalarUDF::new_from_impl(ArrayFn {
        name,
        signature,
        returns,
        kernel,
    }))
}

/// The six registered functions (seven names: `regexp` carries the `match` alias).
///
/// `cache` and `sink` are wired into `regexp` only; the other functions are stateless.
pub fn allowlisted_udfs(cache: Arc<RegexCache>, sink: Arc<LatencySink>) -> Vec<Arc<ScalarUDF>> {
    vec![
        udf(
            "length",
            string_signature(1),
            DataType::Int64,
            length_kernel,
        ),
        udf("instr", string_signature(2), DataType::Int64, instr_kernel),
        udf("hex", binary_signature(), DataType::Utf8, hex_kernel),
        udf("unhex", string_signature(1), DataType::Binary, unhex_kernel),
        udf("like", string_signature(2), DataType::Boolean, like_kernel),
        Arc::new(ScalarUDF::new_from_impl(RegexpUdf::new(cache, sink))),
    ]
}

/// Argument `index` of a kernel call, or an internal error if the planner let the call through
/// with too few arguments (the signatures make that unreachable).
fn arg<'a>(arrays: &'a [ArrayRef], index: usize, name: &str) -> Result<&'a ArrayRef> {
    arrays.get(index).ok_or_else(|| {
        DataFusionError::Internal(format!("{name} received the wrong number of arguments"))
    })
}

fn length_kernel(arrays: &[ArrayRef]) -> Result<ArrayRef> {
    let text = as_utf8(arg(arrays, 0, "length")?)?;
    let out: Int64Array = as_string_array(&text)?
        .iter()
        .map(|cell| cell.map(|value| i64::try_from(value.chars().count()).unwrap_or(i64::MAX)))
        .collect();
    Ok(Arc::new(out))
}

/// 1-based character position of the first `needle` in `haystack`, 0 when absent.
fn char_position(haystack: &str, needle: &str) -> i64 {
    haystack.find(needle).map_or(0, |byte_offset| {
        let chars_before = haystack
            .char_indices()
            .take_while(|&(index, _)| index < byte_offset)
            .count();
        i64::try_from(chars_before)
            .unwrap_or(i64::MAX)
            .saturating_add(1)
    })
}

fn instr_kernel(arrays: &[ArrayRef]) -> Result<ArrayRef> {
    let haystack_array = as_utf8(arg(arrays, 0, "instr")?)?;
    let needle_array = as_utf8(arg(arrays, 1, "instr")?)?;
    let out: Int64Array = as_string_array(&haystack_array)?
        .iter()
        .zip(as_string_array(&needle_array)?.iter())
        .map(|pair| match pair {
            (Some(haystack), Some(needle)) => Some(char_position(haystack, needle)),
            _ => None,
        })
        .collect();
    Ok(Arc::new(out))
}

fn hex_kernel(arrays: &[ArrayRef]) -> Result<ArrayRef> {
    let binary = as_binary(arg(arrays, 0, "hex")?)?;
    let out: StringArray = as_binary_array(&binary)?
        .iter()
        .map(|cell| cell.map(encode_hex))
        .collect();
    Ok(Arc::new(out))
}

const HEX_RADIX: u32 = 16;

fn encode_hex(bytes: &[u8]) -> String {
    let digit = |nibble: u8| char::from_digit(u32::from(nibble), HEX_RADIX).unwrap_or('0');
    bytes
        .iter()
        .flat_map(|byte| [digit(byte >> 4), digit(byte & 0x0f)])
        .collect()
}

/// One byte from two ASCII hex digits, either case; `None` for anything else.
fn hex_pair(high_digit: u8, low_digit: u8) -> Option<u8> {
    let high = char::from(high_digit).to_digit(HEX_RADIX)?;
    let low = char::from(low_digit).to_digit(HEX_RADIX)?;
    u8::try_from(high << 4 | low).ok()
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(DataFusionError::Execution(
            "unhex: input has an odd number of hexadecimal digits".to_owned(),
        ));
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            pair.first()
                .zip(pair.get(1))
                .and_then(|(high, low)| hex_pair(*high, *low))
                .ok_or_else(|| {
                    DataFusionError::Execution(
                        "unhex: input contains a character that is not a hexadecimal digit"
                            .to_owned(),
                    )
                })
        })
        .collect()
}

fn unhex_kernel(arrays: &[ArrayRef]) -> Result<ArrayRef> {
    let text = as_utf8(arg(arrays, 0, "unhex")?)?;
    let strings = as_string_array(&text)?;
    let mut builder = BinaryBuilder::with_capacity(strings.len(), 0);
    for cell in strings {
        match cell {
            Some(hex) => builder.append_value(decode_hex(hex)?),
            None => builder.append_null(),
        }
    }
    Ok(Arc::new(builder.finish()))
}

fn like_kernel(arrays: &[ArrayRef]) -> Result<ArrayRef> {
    let value_array = as_utf8(arg(arrays, 0, "like")?)?;
    let pattern_array = as_utf8(arg(arrays, 1, "like")?)?;
    let out: BooleanArray = arrow_like(
        as_string_array(&value_array)?,
        as_string_array(&pattern_array)?,
    )?;
    Ok(Arc::new(out))
}
