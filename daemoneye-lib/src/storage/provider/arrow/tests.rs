//! Tests for the Arrow encoder.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use crate::models::process::ProcessRecord;
use crate::proto::{
    ColumnDescriptor, ColumnType, METADATA_KEY_ACCESSIBLE, METADATA_KEY_FILE_EXISTS,
    ProcessRecord as ProtoProcessRecord,
};
use chrono::{TimeZone, Utc};
use datafusion::arrow::array::{
    Array, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array,
};
use datafusion::arrow::datatypes::DataType;
use std::time::{Duration, UNIX_EPOCH};

fn col(name: &str, ty: ColumnType, nullable: bool) -> ColumnDescriptor {
    ColumnDescriptor {
        name: name.to_owned(),
        column_type: i32::from(ty),
        nullable,
        supported_ops: vec![],
    }
}

/// Mirrors `procmond::pushdown_eval::process_schema_descriptor`'s `processes` table;
/// `daemoneye-lib` cannot depend on procmond.
fn process_table() -> TableDescriptor {
    TableDescriptor {
        name: "processes".to_owned(),
        columns: vec![
            col("pid", ColumnType::Uint, false),
            col("ppid", ColumnType::Uint, true),
            col("name", ColumnType::String, false),
            col("executable_path", ColumnType::String, true),
            col("command_line", ColumnType::String, true),
            col("start_time", ColumnType::Int, true),
            col("cpu_usage", ColumnType::Float, true),
            col("memory_usage", ColumnType::Uint, true),
            col("executable_hash", ColumnType::String, true),
            col("user_id", ColumnType::String, true),
            col("accessible", ColumnType::Bool, false),
            col("file_exists", ColumnType::Bool, false),
            col("collection_time", ColumnType::Int, false),
        ],
    }
}

fn schema() -> SchemaRef {
    schema_for(&process_table()).unwrap()
}

fn full_record() -> ProcessRecord {
    let mut r = ProcessRecord::new(42, "sshd".to_owned());
    r.ppid = Some(crate::models::ProcessId::new(1));
    r.executable_path = Some("/usr/sbin/sshd".into());
    r.command_line = Some("sshd -D".to_owned());
    r.start_time = Some(
        UNIX_EPOCH
            .checked_add(Duration::from_secs(1_700_000_000))
            .unwrap(),
    );
    r.cpu_usage = Some(1.5);
    r.memory_usage = Some(4096);
    r.executable_hash = Some("abc".to_owned());
    r.user_id = Some(501);
    r.collection_time = Utc.timestamp_millis_opt(1_700_000_123_456).unwrap();
    r
}

fn strs(b: &RecordBatch, i: usize) -> &StringArray {
    b.column(i).as_any().downcast_ref().unwrap()
}
fn bools(b: &RecordBatch, i: usize) -> &BooleanArray {
    b.column(i).as_any().downcast_ref().unwrap()
}
fn i64s(b: &RecordBatch, i: usize) -> &Int64Array {
    b.column(i).as_any().downcast_ref().unwrap()
}
fn u64s(b: &RecordBatch, i: usize) -> &UInt64Array {
    b.column(i).as_any().downcast_ref().unwrap()
}

#[test]
fn schema_has_thirteen_fields_in_descriptor_order_with_mapped_types() {
    let s = schema();
    let got: Vec<(&str, DataType, bool)> = s
        .fields()
        .iter()
        .map(|f| (f.name().as_str(), f.data_type().clone(), f.is_nullable()))
        .collect();
    let want = vec![
        ("pid", DataType::UInt64, false),
        ("ppid", DataType::UInt64, true),
        ("name", DataType::Utf8, false),
        ("executable_path", DataType::Utf8, true),
        ("command_line", DataType::Utf8, true),
        ("start_time", DataType::Int64, true),
        ("cpu_usage", DataType::Float64, true),
        ("memory_usage", DataType::UInt64, true),
        ("executable_hash", DataType::Utf8, true),
        ("user_id", DataType::Utf8, true),
        ("accessible", DataType::Boolean, false),
        ("file_exists", DataType::Boolean, false),
        ("collection_time", DataType::Int64, false),
    ];
    assert_eq!(got, want);
}

#[test]
fn encode_one_record_round_trips_every_column_value() {
    let b = encode(&[full_record()], &schema(), None).unwrap();
    assert_eq!((b.num_rows(), b.num_columns()), (1, 13));
    assert_eq!(u64s(&b, 0).value(0), 42);
    assert_eq!(u64s(&b, 1).value(0), 1);
    assert_eq!(strs(&b, 2).value(0), "sshd");
    assert_eq!(strs(&b, 3).value(0), "/usr/sbin/sshd");
    assert_eq!(strs(&b, 4).value(0), "sshd -D");
    assert_eq!(i64s(&b, 5).value(0), 1_700_000_000);
    let cpu: &Float64Array = b.column(6).as_any().downcast_ref().unwrap();
    assert!((cpu.value(0) - 1.5).abs() < f64::EPSILON);
    assert_eq!(u64s(&b, 7).value(0), 4096);
    assert_eq!(strs(&b, 8).value(0), "abc");
    assert_eq!(strs(&b, 9).value(0), "501");
    assert_eq!(i64s(&b, 12).value(0), 1_700_000_123_456);
}

#[test]
fn every_nullable_column_encodes_none_as_null() {
    let b = encode(&[ProcessRecord::new(7, "bare".to_owned())], &schema(), None).unwrap();
    for (i, f) in schema().fields().iter().enumerate() {
        assert_eq!(b.column(i).is_null(0), f.is_nullable(), "column index {i}");
    }
}

#[test]
fn proto_accessible_false_survives_conversion_into_the_column() {
    let proto = ProtoProcessRecord {
        pid: 1,
        name: "p".to_owned(),
        accessible: false,
        file_exists: true,
        ..Default::default()
    };
    let native = ProcessRecord::from(proto);
    // The key is present: the flag was carried, not defaulted.
    assert_eq!(
        native
            .metadata
            .get(METADATA_KEY_ACCESSIBLE)
            .map(String::as_str),
        Some("false")
    );
    let b = encode(&[native], &schema(), None).unwrap();
    assert!(!bools(&b, 10).value(0));
    // file_exists = true must reach the column; a dropped flag would read false here.
    assert!(bools(&b, 11).value(0));
}

#[test]
fn proto_accessible_true_survives_conversion_into_the_column() {
    let proto = ProtoProcessRecord {
        pid: 1,
        name: "p".to_owned(),
        accessible: true,
        file_exists: false,
        ..Default::default()
    };
    let b = encode(&[ProcessRecord::from(proto)], &schema(), None).unwrap();
    assert!(bools(&b, 10).value(0));
    assert!(!bools(&b, 11).value(0));
}

#[test]
fn absent_flag_keys_default_non_nullable_bools_to_false() {
    let native = ProcessRecord::new(1, "p".to_owned());
    // The keys are absent: false here is a default, not a carried value.
    assert!(!native.metadata.contains_key(METADATA_KEY_ACCESSIBLE));
    assert!(!native.metadata.contains_key(METADATA_KEY_FILE_EXISTS));
    let b = encode(&[native], &schema(), None).unwrap();
    assert!(!bools(&b, 10).value(0));
    assert!(!bools(&b, 11).value(0));
}

#[test]
fn absent_flag_is_null_when_the_descriptor_makes_the_bool_nullable() {
    let mut t = process_table();
    t.columns.get_mut(10).unwrap().nullable = true;
    let s = schema_for(&t).unwrap();
    let b = encode(&[ProcessRecord::new(1, "p".to_owned())], &s, None).unwrap();
    assert!(b.column(10).is_null(0));
}

#[test]
fn unknown_column_fails_construction() {
    let mut t = process_table();
    t.columns.push(col("unknown_col", ColumnType::String, true));
    match schema_for(&t) {
        Err(ArrowEncodeError::UnbackedColumn(name)) => assert_eq!(name, "unknown_col"),
        other => panic!("expected UnbackedColumn, got {other:?}"),
    }
}

#[test]
fn declared_type_disagreeing_with_the_encoder_fails_construction() {
    let mut t = process_table();
    t.columns.get_mut(0).unwrap().column_type = i32::from(ColumnType::Int);
    assert!(matches!(
        schema_for(&t),
        Err(ArrowEncodeError::TypeMismatch { ref column, .. }) if column == "pid"
    ));
}

#[test]
fn unspecified_and_unrecognized_column_types_fail_construction() {
    for raw in [0, 99] {
        let mut t = process_table();
        t.columns.get_mut(2).unwrap().column_type = raw;
        assert!(matches!(
            schema_for(&t),
            Err(ArrowEncodeError::UnknownColumnType { ref column, raw: r }) if column == "name" && r == raw
        ));
    }
}

#[test]
fn projection_selects_and_orders_columns() {
    let recs = [full_record()];
    let b = encode(&recs, &schema(), Some(&[2, 0])).unwrap();
    assert_eq!(
        b.schema()
            .fields()
            .iter()
            .map(|f| f.name().as_str())
            .collect::<Vec<_>>(),
        ["name", "pid"]
    );
    assert_eq!(strs(&b, 0).value(0), "sshd");
    assert_eq!(u64s(&b, 1).value(0), 42);
}

#[test]
fn projected_batch_row_count_equals_input_length() {
    let recs: Vec<_> = (0..3)
        .map(|i| ProcessRecord::new(i, format!("p{i}")))
        .collect();
    let b = encode(&recs, &schema(), Some(&[0])).unwrap();
    assert_eq!(b.num_rows(), 3);
    assert_eq!(u64s(&b, 0).values(), &[0, 1, 2]);
}

#[test]
fn empty_projection_keeps_the_row_count() {
    let recs = [full_record(), full_record()];
    let b = encode(&recs, &schema(), Some(&[])).unwrap();
    assert_eq!((b.num_rows(), b.num_columns()), (2, 0));
}

#[test]
fn repeated_projection_index_yields_the_column_twice() {
    let b = encode(&[full_record()], &schema(), Some(&[0, 0])).unwrap();
    assert_eq!(b.num_columns(), 2);
    assert_eq!(u64s(&b, 0).value(0), 42);
    assert_eq!(u64s(&b, 1).value(0), 42);
}

#[test]
fn out_of_range_projection_is_an_error_not_a_panic() {
    assert!(matches!(
        encode(&[full_record()], &schema(), Some(&[0, 13])),
        Err(ArrowEncodeError::ProjectionOutOfRange {
            index: 13,
            width: 13
        })
    ));
}

#[test]
fn empty_slice_encodes_a_zero_row_batch_with_the_projected_schema() {
    let b = encode(&[], &schema(), Some(&[2, 0])).unwrap();
    assert_eq!((b.num_rows(), b.num_columns()), (0, 2));
    assert_eq!(b.schema().field(0).name(), "name");
    let full = encode(&[], &schema(), None).unwrap();
    assert_eq!((full.num_rows(), full.num_columns()), (0, 13));
}

#[cfg(unix)]
#[test]
fn non_utf8_executable_path_is_lossily_rendered_not_nulled() {
    use std::os::unix::ffi::OsStrExt;
    let mut r = ProcessRecord::new(1, "p".to_owned());
    r.executable_path = Some(std::path::PathBuf::from(std::ffi::OsStr::from_bytes(
        b"/bin/\xFFx",
    )));
    let b = encode(&[r], &schema(), Some(&[3])).unwrap();
    assert_eq!(strs(&b, 0).value(0), "/bin/\u{FFFD}x");
}

#[test]
fn pre_epoch_start_time_is_negative_seconds() {
    let mut r = ProcessRecord::new(1, "p".to_owned());
    r.start_time = Some(UNIX_EPOCH.checked_sub(Duration::from_secs(10)).unwrap());
    let b = encode(&[r], &schema(), Some(&[5])).unwrap();
    assert_eq!(i64s(&b, 0).value(0), -10);
}

#[test]
fn fractional_pre_epoch_start_time_floors() {
    assert_eq!(
        epoch_seconds(UNIX_EPOCH.checked_sub(Duration::from_millis(1500)).unwrap()),
        Some(-2)
    );
    assert_eq!(
        epoch_seconds(UNIX_EPOCH.checked_add(Duration::from_millis(1500)).unwrap()),
        Some(1)
    );
}

#[test]
fn start_time_beyond_i64_seconds_is_an_error() {
    // How far before the epoch a `SystemTime` can sit is platform-defined, so both
    // outcomes assert. A bare early return here would make this test pass while
    // checking nothing on any platform that cannot represent the value, and CI is a
    // three-OS matrix. (Representable on macOS aarch64, where the first branch runs.)
    if let Some(far) = UNIX_EPOCH.checked_sub(Duration::new(i64::MAX.unsigned_abs() + 1, 0)) {
        assert_eq!(epoch_seconds(far), None);
        let mut r = ProcessRecord::new(1, "p".to_owned());
        r.start_time = Some(far);
        assert!(matches!(
            encode(&[r], &schema(), Some(&[5])),
            Err(ArrowEncodeError::StartTimeOutOfRange)
        ));
    } else {
        // The out-of-range case is unreachable here, so assert the boundary that
        // is: the furthest representable pre-epoch instant still encodes.
        let near = UNIX_EPOCH
            .checked_sub(Duration::new(1_u64 << 62, 0))
            .expect("2^62 seconds before the epoch is representable");
        assert!(epoch_seconds(near).is_some());
        let mut r = ProcessRecord::new(1, "p".to_owned());
        r.start_time = Some(near);
        assert!(encode(&[r], &schema(), Some(&[5])).is_ok());
    }
}

#[test]
fn collection_time_millis_matches_for_pre_epoch_dates() {
    let mut r = ProcessRecord::new(1, "p".to_owned());
    r.collection_time = Utc.timestamp_millis_opt(-1500).unwrap();
    let b = encode(&[r], &schema(), Some(&[12])).unwrap();
    assert_eq!(i64s(&b, 0).value(0), -1500);
}

#[test]
fn null_in_a_non_nullable_column_is_an_error() {
    let mut t = process_table();
    t.columns.get_mut(1).unwrap().nullable = false; // ppid
    let s = schema_for(&t).unwrap();
    let r = encode(&[ProcessRecord::new(1, "p".to_owned())], &s, None);
    assert!(matches!(r, Err(ArrowEncodeError::Arrow(_))));
}
