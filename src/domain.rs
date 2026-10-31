//! Domain contracts and validation. All chat text is untrusted data, never authority.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Failure {
    pub code: &'static str,
    pub message: &'static str,
}
impl Failure {
    pub const fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }
    pub const fn backend() -> Self {
        Self::new(
            "relay_unavailable",
            "The private Relay is unavailable; no delivery is implied.",
        )
    }
    pub const fn denied() -> Self {
        Self::new("access_denied", "This operation is not permitted.")
    }
    pub const fn shape() -> Self {
        Self::new(
            "relay_contract",
            "Relay returned an unsupported or invalid response.",
        )
    }
}
pub type Result<T> = std::result::Result<T, Failure>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub local_id: String,
    pub server_msgid: Option<String>,
    pub server_time: Option<String>,
    pub observed_time: String,
    pub nick: Option<String>,
    pub account: Option<String>,
    pub text: String,
    pub highlight: bool,
    pub trust: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RelayLine {
    pub id: u64,
    pub date: String,
    #[serde(default)]
    pub prefix: String,
    pub message: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub highlight: bool,
}
impl RelayLine {
    pub fn message(&self) -> Option<Message> {
        if !self
            .tags
            .iter()
            .any(|t| t == "irc_privmsg" || t == "irc_notice")
        {
            return None;
        }
        let tag = |prefix: &str| {
            let mut values = self.tags.iter().filter_map(|t| t.strip_prefix(prefix));
            let first = values.next()?;
            // Conflicting or oversized metadata is unknown, never invented identity.
            if values.next().is_some()
                || first.is_empty()
                || first.len() > 256
                || first.chars().any(char::is_control)
            {
                return None;
            }
            Some(first.to_owned())
        };
        Some(Message {
            local_id: self.id.to_string(),
            server_msgid: tag("irc_tag_msgid="),
            server_time: tag("irc_tag_time="),
            observed_time: self.date.chars().take(64).collect(),
            nick: tag("nick_"),
            account: tag("irc_tag_account=").filter(|a| a != "*"),
            text: self.message.clone(),
            highlight: self.highlight,
            trust: "untrusted_chat".into(),
        })
    }
}
/// Fail closed instead of relying on untested command-escaping conventions.
pub fn validate_text(text: &str) -> Result<()> {
    if text.trim().is_empty()
        || text.len() > 400
        || text.chars().any(char::is_control)
        || text.trim_start().starts_with('/')
    {
        return Err(Failure::new("invalid_message", "Use 1–400 UTF-8 bytes of single-line text, without control characters or a leading slash."));
    }
    Ok(())
}
pub fn valid_key(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn validate_request_key(s: &str) -> Result<()> {
    if !(16..=128).contains(&s.len())
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Failure::new("invalid_request_key", "Supply a unique 16–128 character request key using letters, digits, hyphen or underscore."));
    }
    Ok(())
}
/// Conservative initial target grammar. No commas, spaces, flags or raw command fields.
pub fn valid_irc_target(s: &str) -> bool {
    let body = s.strip_prefix('#').unwrap_or(s);
    valid_key(body)
        && (s.starts_with('#') || body.as_bytes().first().is_some_and(u8::is_ascii_alphabetic))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_command_and_frame_injection() {
        for input in [
            "/quit",
            " /exec whoami",
            "\r\nPRIVMSG #x :x",
            "hello\0",
            "\t/quit",
            "",
            "   ",
        ] {
            assert!(validate_text(input).is_err(), "{input:?}");
        }
        assert!(validate_text("Hello; /quit is just text here").is_ok());
    }
    #[test]
    fn enforces_utf8_byte_budget() {
        assert!(validate_text(&"é".repeat(200)).is_ok());
        assert!(validate_text(&"é".repeat(201)).is_err());
    }
    #[test]
    fn strict_target_grammar() {
        for s in ["#ops", "#runner-platform", "helper1"] {
            assert!(valid_irc_target(s));
        }
        for s in ["#", "-server", "#a,#b", "a b", "a\r\n", "/exec"] {
            assert!(!valid_irc_target(s));
        }
    }
    #[test]
    fn structured_message_does_not_invent_identity() {
        let line = RelayLine {
            id: 1,
            date: "2026-01-01T00:00:00Z".into(),
            prefix: "@admin".into(),
            message: "hello".into(),
            tags: vec!["irc_privmsg".into()],
            highlight: false,
        };
        let message = line.message().unwrap();
        assert_eq!(message.nick, None);
        assert_eq!(message.account, None);
        assert_eq!(message.server_msgid, None);
    }
    #[test]
    fn metadata_tags_use_weechat_equals_encoding() {
        let line = RelayLine {
            id: 12,
            date: "2026-10-31T12:00:00Z".into(),
            prefix: String::new(),
            message: "hello".into(),
            tags: vec![
                "irc_privmsg".into(),
                "irc_tag_account=alice".into(),
                "irc_tag_time=2026-10-31T12:00:00Z".into(),
                "irc_tag_msgid=abcdef".into(),
            ],
            highlight: false,
        };
        let message = line.message().expect("message");
        assert_eq!(message.account.as_deref(), Some("alice"));
        assert_eq!(message.server_time.as_deref(), Some("2026-10-31T12:00:00Z"));
        assert_eq!(message.server_msgid.as_deref(), Some("abcdef"));
    }
    #[test]
    fn tags_survive_as_data_not_credentials() {
        let line: RelayLine =
            serde_json::from_str(include_str!("../tests/fixtures/line.json")).unwrap();
        let m = line.message().unwrap();
        assert_eq!(m.nick.as_deref(), Some("helper"));
        assert_eq!(m.account.as_deref(), Some("helper-account"));
        assert_eq!(m.trust, "untrusted_chat");
    }
}
