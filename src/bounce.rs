use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BounceClass {
    Success,
    Defer,
    Soft,
    Hard,
    Block,
}

impl BounceClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Defer => "defer",
            Self::Soft => "soft",
            Self::Hard => "hard",
            Self::Block => "block",
        }
    }

    pub fn is_failure(self) -> bool {
        matches!(self, Self::Hard | Self::Block)
    }
}

pub fn classify_smtp(code: u16, enhanced: Option<&str>, text: &str) -> BounceClass {
    if (200..300).contains(&code) {
        return BounceClass::Success;
    }

    let text_l = text.to_ascii_lowercase();
    let enh = enhanced.unwrap_or("").to_ascii_lowercase();
    let blob = format!("{enh} {text_l}");

    if is_block(code, &blob) {
        return BounceClass::Block;
    }
    if is_hard(&blob) {
        return BounceClass::Hard;
    }
    if (400..500).contains(&code) || is_soft(&blob) {
        if is_soft(&blob) && (500..600).contains(&code) {
            return BounceClass::Soft;
        }
        return BounceClass::Defer;
    }
    if (500..600).contains(&code) {
        return BounceClass::Hard;
    }
    BounceClass::Defer
}

fn is_block(code: u16, blob: &str) -> bool {
    if blob.contains("5.7.1")
        || blob.contains("5.7.0")
        || blob.contains("5.7.26")
        || blob.contains("blocked")
        || blob.contains("blacklist")
        || blob.contains("blocklist")
        || blob.contains("spamhaus")
        || blob.contains("listed on")
        || blob.contains("reputation")
        || (blob.contains("spam") && blob.contains("reject"))
        || blob.contains("not allowed to send")
        || blob.contains("access denied")
        || blob.contains("policy violation")
    {
        return true;
    }
    code == 421 && blob.contains("4.7.")
}

fn is_hard(blob: &str) -> bool {
    blob.contains("5.1.1")
        || blob.contains("5.1.10")
        || blob.contains("user unknown")
        || blob.contains("unknown user")
        || blob.contains("does not exist")
        || blob.contains("doesn't exist")
        || blob.contains("no such user")
        || blob.contains("recipient address rejected")
        || blob.contains("mailbox unavailable") && blob.contains("5.1")
        || blob.contains("invalid recipient")
        || blob.contains("bad destination")
        || blob.contains("not a valid mailbox")
}

fn is_soft(blob: &str) -> bool {
    blob.contains("mailbox full")
        || blob.contains("over quota")
        || blob.contains("insufficient storage")
        || blob.contains("out of storage")
        || blob.contains("4.2.2")
        || blob.contains("5.2.2")
}

pub fn parse_verp_local_part(local: &str) -> Option<&str> {
    local
        .strip_prefix("bounce+")
        .or_else(|| local.strip_prefix("bounce-"))
        .filter(|id| !id.is_empty() && id.len() <= 80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivered_2xx() {
        assert_eq!(
            classify_smtp(250, Some("2.0.0"), "OK"),
            BounceClass::Success
        );
    }

    #[test]
    fn greylist_defers() {
        assert_eq!(
            classify_smtp(451, Some("4.7.1"), "greylisted, try again"),
            BounceClass::Defer
        );
    }

    #[test]
    fn user_unknown_is_hard() {
        assert_eq!(
            classify_smtp(550, Some("5.1.1"), "User unknown"),
            BounceClass::Hard
        );
    }

    #[test]
    fn spamhaus_is_block() {
        assert_eq!(
            classify_smtp(550, Some("5.7.1"), "Blocked by Spamhaus"),
            BounceClass::Block
        );
    }

    #[test]
    fn mailbox_full_is_soft() {
        assert_eq!(
            classify_smtp(552, Some("5.2.2"), "Mailbox full"),
            BounceClass::Soft
        );
    }

    #[test]
    fn verp_extracts_id() {
        assert_eq!(parse_verp_local_part("bounce+abc-123"), Some("abc-123"));
        assert_eq!(parse_verp_local_part("nobody"), None);
    }
}
