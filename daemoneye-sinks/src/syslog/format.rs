//! RFC 5424 message formatting from the external projection.

use chrono::{DateTime, Utc};
use daemoneye_lib::alerting::{ExternalAlert, IncludeFlags, sanitise_control};
use daemoneye_lib::models::{Alert, AlertSeverity, CompletenessStatus};

/// 32473 is the IANA private-enterprise number reserved for documentation.
const SD_ID: &str = "daemoneye@32473";
const DEFAULT_FACILITY: u8 = 3;
/// RFC 5424 allows 255, but truncation is permitted; 64 keeps the worst-case flagged line under
/// the 480-byte `max_message_bytes` floor (about 461 bytes with every field at its cap).
const MAX_HOSTNAME: usize = 64;
const MAX_APP_NAME: usize = 48;
const MAX_MSGID: usize = 32;
/// Bounds each SD-PARAM value (up to twice that once escaped) so the flagged line fits the
/// 480-byte size floor.
const MAX_SD_VALUE_CHARS: usize = 48;
const TRUNCATED_PARAM: &str = " truncated=\"1\"";

/// Everything the formatter needs for one message.
pub(super) struct MessageParts<'a> {
    pub facility: u8,
    pub app_name: &'a str,
    pub max_bytes: usize,
    pub flags: IncludeFlags,
    pub alert: &'a Alert,
}

/// Facility number for a syslog facility name; unknown names map to `daemon`.
pub(super) fn parse_facility(name: &str) -> u8 {
    const NAMED: [&str; 12] = [
        "kern", "user", "mail", "daemon", "auth", "syslog", "lpr", "news", "uucp", "cron",
        "authpriv", "ftp",
    ];
    let lower = name.trim().to_ascii_lowercase();
    if let Some(index) = NAMED.iter().position(|known| *known == lower) {
        return u8::try_from(index).unwrap_or(DEFAULT_FACILITY);
    }
    lower
        .strip_prefix("local")
        .and_then(|digit| digit.parse::<u8>().ok())
        .filter(|digit| *digit <= 7)
        .map_or(DEFAULT_FACILITY, |digit| digit.saturating_add(16))
}

#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "AlertSeverity is non_exhaustive; unknown levels log as the lowest"
)]
const fn severity_code(severity: AlertSeverity) -> u8 {
    match severity {
        AlertSeverity::Critical => 2,
        AlertSeverity::High => 3,
        AlertSeverity::Medium => 4,
        _ => 5,
    }
}

/// Printable ASCII only, bounded; empty becomes the NILVALUE.
fn header_field(value: &str, max: usize) -> String {
    let text: String = value
        .chars()
        .take(max)
        .map(|c| if c.is_ascii_graphic() { c } else { '_' })
        .collect();
    if text.is_empty() {
        "-".to_owned()
    } else {
        text
    }
}

/// Control characters stripped, bounded, then escaped per RFC 5424 section 6.3.3.
fn sd_value(value: &str) -> String {
    sanitise_control(value)
        .chars()
        .take(MAX_SD_VALUE_CHARS)
        .fold(String::new(), |mut out, c| {
            if matches!(c, '\\' | '"' | ']') {
                out.push('\\');
            }
            out.push(c);
            out
        })
}

fn timestamp(millis: u64) -> String {
    i64::try_from(millis)
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .map_or_else(
            || "-".to_owned(),
            |time| time.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        )
}

#[allow(
    clippy::wildcard_enum_match_arm,
    reason = "CompletenessStatus is non_exhaustive"
)]
const fn completeness_label(status: CompletenessStatus) -> &'static str {
    match status {
        CompletenessStatus::Degraded => "degraded",
        _ => "complete",
    }
}

fn summary(external: &ExternalAlert) -> String {
    // Opt-in fields ride at the end of MSG, where truncation drops them first.
    let user = external
        .user
        .as_deref()
        .map(|user| format!(" user={user}"))
        .unwrap_or_default();
    let command_line = external
        .command_line
        .as_deref()
        .map(|cmd| format!(" cmd={cmd}"))
        .unwrap_or_default();
    sanitise_control(&format!(
        "[{}] {} - {}: {}{user}{command_line}",
        external.severity, external.rule_id, external.title, external.description
    ))
}

/// Longest prefix of `text` that fits in `budget` bytes, cut on a char boundary.
fn fit(text: &str, budget: usize) -> &str {
    let end = text
        .char_indices()
        .map(|(index, c)| index.saturating_add(c.len_utf8()))
        .take_while(|end| *end <= budget)
        .last()
        .unwrap_or(0);
    text.get(..end).unwrap_or("")
}

/// Build one RFC 5424 line, truncating the free-text MSG to honour `max_bytes`.
pub(super) fn format_rfc5424(parts: &MessageParts<'_>) -> String {
    let alert = parts.alert;
    let external = ExternalAlert::project(alert, parts.flags);
    let header = format!(
        "<{}>1 {} {} {} - {}",
        u16::from(parts.facility)
            .saturating_mul(8)
            .saturating_add(u16::from(severity_code(alert.severity))),
        timestamp(external.timestamp_ms),
        header_field(external.host.as_deref().unwrap_or(""), MAX_HOSTNAME),
        header_field(parts.app_name, MAX_APP_NAME),
        header_field(&external.id.replace('-', ""), MAX_MSGID),
    );
    let params = format!(
        " rule=\"{}\" pid=\"{}\" completeness=\"{}\" dedup=\"{}\"",
        sd_value(&external.rule_id),
        external.pid,
        completeness_label(external.completeness.status()),
        sd_value(&alert.deduplication_key),
    );
    let line = |flag: &str, msg: &str| {
        let sep = if msg.is_empty() { "" } else { " " };
        format!("{header} [{SD_ID}{params}{flag}]{sep}{msg}")
    };
    let msg = summary(&external);
    let full = line("", &msg);
    if full.len() <= parts.max_bytes {
        return full;
    }
    // The flagged line costs one separator byte on top of its empty-MSG form.
    let budget = parts
        .max_bytes
        .saturating_sub(line(TRUNCATED_PARAM, "").len().saturating_add(1));
    let truncated = line(TRUNCATED_PARAM, fit(&msg, budget));
    // Backstop: the caps above keep this unreachable, but never exceed the datagram cap.
    fit(&truncated, parts.max_bytes).to_owned()
}
