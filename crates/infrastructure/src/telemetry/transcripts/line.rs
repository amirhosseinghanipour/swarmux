//! One transcript line → the usage numbers of an assistant message.
//!
//! Only `type`, timestamps, ids, `cwd`, the model and `usage` are read;
//! message content is skipped unread.

use serde::Deserialize;
use swarmux_domain::Millis;
use swarmux_domain::parse_iso_millis;
use swarmux_domain::telemetry::TokenUsage;

/// The numbers of one assistant message.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageLine {
    /// Message id + request id, hashed: the same message written twice
    /// (streamed blocks, resumed sessions) keeps one key.
    pub key: u64,
    pub ts: Millis,
    pub session: String,
    pub cwd: String,
    pub model: String,
    pub sidechain: bool,
    pub usage: TokenUsage,
}

#[derive(Deserialize)]
struct Line {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "isSidechain", default)]
    sidechain: bool,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    cache_creation: Option<CacheCreation>,
}

#[derive(Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

/// The usage of an assistant line; `None` for every other line, broken
/// lines and messages without tokens.
pub fn parse_usage_line(line: &[u8]) -> Option<UsageLine> {
    // Cheap filter first: most lines are user turns, tool output or metadata.
    let text = std::str::from_utf8(line).ok()?;
    if !text.contains("\"usage\"") || !text.contains("\"assistant\"") {
        return None;
    }
    let l: Line = serde_json::from_str(text).ok()?;
    if l.kind.as_deref() != Some("assistant") {
        return None;
    }
    let msg = l.message?;
    let u = msg.usage?;
    let (w5, w1) = match u.cache_creation {
        Some(c) if c.ephemeral_5m_input_tokens + c.ephemeral_1h_input_tokens > 0 => {
            (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens)
        }
        _ => (u.cache_creation_input_tokens, 0),
    };
    let usage = TokenUsage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cache_read_input_tokens,
        cache_write_5m: w5,
        cache_write_1h: w1,
    };
    if usage.total() == 0 {
        return None;
    }
    let ts = parse_iso_millis(l.timestamp.as_deref()?)?;
    let session = l.session_id.unwrap_or_default();
    let key = match (&msg.id, &l.request_id) {
        (Some(id), req) => fnv1a(&[id.as_bytes(), b":", req.as_deref().unwrap_or("").as_bytes()]),
        // No id to dedupe by: the session and time identify it.
        (None, _) => fnv1a(&[session.as_bytes(), b"@", &ts.to_le_bytes()]),
    };
    Some(UsageLine {
        key,
        ts,
        session,
        cwd: l.cwd.unwrap_or_default(),
        model: msg.model.unwrap_or_default(),
        sidechain: l.sidechain,
        usage,
    })
}

/// 64-bit FNV-1a: stable across runs and Rust versions.
fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in parts.iter().flat_map(|p| p.iter()) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSISTANT: &str = r#"{"parentUuid":"p","isSidechain":false,"type":"assistant","cwd":"/work/demo","sessionId":"sess-1","version":"2.1.316","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"SECRET"}],"usage":{"input_tokens":2,"cache_creation_input_tokens":41225,"cache_read_input_tokens":37233,"output_tokens":214,"cache_creation":{"ephemeral_1h_input_tokens":41225,"ephemeral_5m_input_tokens":0}}},"requestId":"req_1","timestamp":"2026-11-01T21:51:00.454Z","uuid":"u"}"#;

    #[test]
    fn reads_usage_model_and_ids() {
        let u = parse_usage_line(ASSISTANT.as_bytes()).unwrap();
        assert_eq!(
            (u.session.as_str(), u.cwd.as_str()),
            ("sess-1", "/work/demo")
        );
        assert_eq!(u.model, "claude-opus-5-5");
        assert_eq!(u.ts, parse_iso_millis("2026-11-01T21:51:00.454Z").unwrap());
        assert_eq!(
            u.usage,
            TokenUsage {
                input: 2,
                output: 214,
                cache_read: 37_233,
                cache_write_5m: 0,
                cache_write_1h: 41_225,
            }
        );
        assert!(!u.sidechain);
        assert_eq!(u.key, fnv1a(&[b"msg_1", b":", b"req_1"]));
    }

    #[test]
    fn older_lines_without_the_ttl_split_count_as_five_minutes() {
        let line = ASSISTANT.replace(
            r#","cache_creation":{"ephemeral_1h_input_tokens":41225,"ephemeral_5m_input_tokens":0}"#,
            "",
        );
        let u = parse_usage_line(line.as_bytes()).unwrap();
        assert_eq!(
            (u.usage.cache_write_5m, u.usage.cache_write_1h),
            (41_225, 0)
        );
    }

    #[test]
    fn other_lines_are_skipped() {
        let user = r#"{"type":"user","message":{"role":"user","content":"usage of assistant"},"timestamp":"2026-11-01T21:51:00Z"}"#;
        assert_eq!(parse_usage_line(user.as_bytes()), None);
        assert_eq!(parse_usage_line(b"{broken \"usage\" \"assistant\""), None);
        assert_eq!(parse_usage_line(b""), None);
        let zero = ASSISTANT
            .replace("\"input_tokens\":2", "\"input_tokens\":0")
            .replace("41225", "0")
            .replace("37233", "0")
            .replace("214", "0");
        assert_eq!(parse_usage_line(zero.as_bytes()), None);
        let no_time = ASSISTANT.replace(r#""timestamp":"2026-11-01T21:51:00.454Z","#, "");
        assert_eq!(parse_usage_line(no_time.as_bytes()), None);
    }
}
