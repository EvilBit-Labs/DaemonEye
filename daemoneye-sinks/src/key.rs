//! Wire form of the receiver deduplication key.

use ring::digest::{SHA256, digest};
use std::borrow::Cow;

/// A key that is printable ASCII without `"`, `\` or `]` and fits `max_bytes` is sent as
/// is. Any other key is sent as `sha256:` plus the hex digest of the whole key, so two
/// distinct keys never share a wire value.
pub fn wire_key(key: &str, max_bytes: usize) -> Cow<'_, str> {
    let plain = key.len() <= max_bytes
        && key
            .chars()
            .all(|c| (c.is_ascii_graphic() || c == ' ') && !matches!(c, '"' | '\\' | ']'));
    if plain {
        return Cow::Borrowed(key);
    }
    let hex: String = digest(&SHA256, key.as_bytes())
        .as_ref()
        .iter()
        .flat_map(|byte| [byte >> 4_u8, byte & 0x0f])
        .map(|nibble| char::from_digit(u32::from(nibble), 16).unwrap_or('0'))
        .collect();
    Cow::Owned(format!("sha256:{hex}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::wire_key;

    #[test]
    fn plain_keys_pass_through_and_the_rest_are_digested() {
        assert_eq!(wire_key("high:rule:title", 32), "high:rule:title");
        let long = "k".repeat(33);
        let digested = wire_key(&long, 32);
        assert_eq!(digested.len(), 71);
        assert!(digested.starts_with("sha256:"));
        assert_ne!(digested, wire_key(&"k".repeat(34), 32));
        assert!(wire_key("a\"b", 32).starts_with("sha256:"));
        assert!(wire_key("caf\u{e9}", 32).starts_with("sha256:"));
    }
}
