//! RFC 5424 message formatting from the external projection.

use crate::key::wire_key;
use chrono::{DateTime, Utc};
use daemoneye_lib::alerting::{ExternalAlert, IncludeFlags, sanitise_control};
use daemoneye_lib::models::{Alert, AlertSeverity, CompletenessStatus};

/// 32473 is the IANA private-enterprise number reserved for documentation.
const SD_ID: &str = "daemoneye@32473";
const DEFAULT_FACILITY: u8 = 3;
/// RFC 5424 allows 255, but truncation is permitted. Header fields are printable ASCII, so these
/// character caps are also byte caps.
const MAX_HOSTNAME: usize = 64;
const MAX_APP_NAME: usize = 48;
const MAX_MSGID: usize = 32;
/// Bounds each escaped SD-PARAM value in UTF-8 bytes. Worst-case empty-MSG flagged line, in bytes:
/// header 180 (PRI 7, timestamp 25, host 65, app 49, `-` and 2 spaces, msgid 32) + SD id 17 +
/// fixed params 58 (`rule`, `pid` at 10 digits, `completeness="degraded"`, `dedup`) +
/// `truncated="1"` 14 + `]` and MSG separator 2 = 271, plus two values at the cap:
/// 271 + 2 * 96 = 463, under the 480-byte `max_message_bytes` floor.
const MAX_SD_VALUE_BYTES: usize = 96;
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

/// Control characters stripped, escaped per RFC 5424 section 6.3.3, then bounded by escaped UTF-8
/// byte length. Whole characters only: a multi-byte char or its escape backslash is never split.
fn sd_value(value: &str) -> String {
    let mut out = String::new();
    for c in sanitise_control(value).chars() {
        let escaped = if matches!(c, '\\' | '"' | ']') { 2 } else { 0 };
        let width = c.len_utf8().saturating_add(escaped);
        if out.len().saturating_add(width) > MAX_SD_VALUE_BYTES {
            break;
        }
        if escaped > 0 {
            out.push('\\');
        }
        out.push(c);
    }
    out
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
    let rule = sd_value(&external.rule_id);
    let dedup = sd_value(&wire_key(&alert.deduplication_key, MAX_SD_VALUE_BYTES));
    let params = |with_rule: bool, with_dedup: bool| {
        let rule_param = if with_rule {
            format!(" rule=\"{rule}\"")
        } else {
            String::new()
        };
        let dedup_param = if with_dedup {
            format!(" dedup=\"{dedup}\"")
        } else {
            String::new()
        };
        format!(
            "{rule_param} pid=\"{}\" completeness=\"{}\"{dedup_param}",
            external.pid,
            completeness_label(external.completeness.status()),
        )
    };
    let line = |sd_params: &str, flag: &str, msg: &str| {
        let sep = if msg.is_empty() { "" } else { " " };
        format!("{header} [{SD_ID}{sd_params}{flag}]{sep}{msg}")
    };
    let msg = summary(&external);
    let full = line(&params(true, true), "", &msg);
    if full.len() <= parts.max_bytes {
        return full;
    }
    // Defensive: the byte caps keep the first variant under the floor. If a header ever grows past
    // it, drop optional SD params (dedup, then rule) rather than cut inside an element.
    let fixed = [(true, true), (true, false), (false, false)]
        .into_iter()
        .map(|(with_rule, with_dedup)| params(with_rule, with_dedup))
        .find(|candidate| line(candidate, TRUNCATED_PARAM, "").len() <= parts.max_bytes)
        .unwrap_or_else(|| params(false, false));
    // The flagged line costs one separator byte on top of its empty-MSG form.
    let budget = parts
        .max_bytes
        .saturating_sub(line(&fixed, TRUNCATED_PARAM, "").len().saturating_add(1));
    let truncated = line(&fixed, TRUNCATED_PARAM, fit(&msg, budget));
    // Last resort, unreachable at the 480-byte floor: never exceed the datagram cap.
    fit(&truncated, parts.max_bytes).to_owned()
}
