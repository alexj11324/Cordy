//! Capability bitmask a Channel uses to DECLARE what it supports.
//!
//! Port of `server/internal/integrations/channel/capability.go`. It is
//! declaration only: this crate contains no degrade logic. A caller that
//! wants to degrade output (rich card → plain text when
//! [`Capability::RICH_CARD`] is absent) reads the bitmask and decides for
//! itself, so adding a new platform never forces a branch into the core.
//! [`crate::Channel::capabilities`] returns a Channel's fixed set; the
//! zero value declares nothing.

/// A set of declared channel capabilities (bitmask).
///
/// Port note: Go uses `1 << iota` constants; Rust uses associated consts
/// over a plain `u64` newtype so the bit layout stays identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Capability(pub u64);

impl Capability {
    /// Can deliver a plain text message. Every Channel is expected to
    /// declare at least this.
    pub const TEXT: Capability = Capability(1 << 0);
    /// Can render a rich / interactive card (Lark interactive card,
    /// Slack Block Kit, …).
    pub const RICH_CARD: Capability = Capability(1 << 1);
    /// Can post a reply into a thread / topic.
    pub const THREAD_REPLY: Capability = Capability(1 << 2);
    /// Can quote-reply to a specific message.
    pub const QUOTE_REPLY: Capability = Capability(1 << 3);
    /// Can send and/or receive media attachments.
    pub const ATTACHMENT: Capability = Capability(1 << 4);
    /// Can handle voice / audio messages.
    pub const VOICE: Capability = Capability(1 << 5);
    /// Can show a typing / "thinking" indicator.
    pub const TYPING_INDICATOR: Capability = Capability(1 << 6);
    /// Can edit a message after it was sent (Lark card patch, Slack
    /// chat.update, …).
    pub const MESSAGE_EDIT: Capability = Capability(1 << 7);

    /// Reports whether `self` declares every capability in `want`.
    /// `has(Capability(0))` is true (the empty requirement is always
    /// satisfied). Because `want` may be a combination of bits, this is
    /// an "includes all of" test, not "any of".
    pub fn has(self, want: Capability) -> bool {
        self.0 & want.0 == want.0
    }

    /// Renders the set bits as a "|"-joined list of names
    /// ("text|thread_reply"), "none" for the zero value, and appends any
    /// unknown high bits as a hex remainder so a forgotten name never
    /// silently vanishes from logs. Diagnostics only; output matches the
    /// Go String() byte-for-byte.
    pub fn render(self) -> String {
        if self.0 == 0 {
            return "none".to_string();
        }
        // Order matches the bit order above so the rendering reads
        // least-significant-bit first.
        const NAMES: [(u64, &str); 8] = [
            (Capability::TEXT.0, "text"),
            (Capability::RICH_CARD.0, "rich_card"),
            (Capability::THREAD_REPLY.0, "thread_reply"),
            (Capability::QUOTE_REPLY.0, "quote_reply"),
            (Capability::ATTACHMENT.0, "attachment"),
            (Capability::VOICE.0, "voice"),
            (Capability::TYPING_INDICATOR.0, "typing_indicator"),
            (Capability::MESSAGE_EDIT.0, "message_edit"),
        ];
        let mut parts: Vec<&str> = Vec::new();
        let mut remaining = self.0;
        for (bit, name) in NAMES {
            if remaining & bit == bit {
                parts.push(name);
                remaining &= !bit;
            }
        }
        let mut out = parts.join("|");
        if remaining != 0 {
            // Fixed lower-case hex without leading-zero trimming beyond
            // Go's TrimLeft("0") — but a value that trims to empty keeps
            // one digit ("0x0" cannot occur here since remaining != 0).
            let hexed = format!("{remaining:x}");
            let trimmed = hexed.trim_start_matches('0');
            let trimmed = if trimmed.is_empty() { "0" } else { trimmed };
            if !out.is_empty() {
                out.push('|');
            }
            out.push_str("0x");
            out.push_str(trimmed);
        }
        out
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

/// Bitwise-OR composition: `TEXT | RICH_CARD`.
impl std::ops::BitOr for Capability {
    type Output = Capability;
    fn bitor(self, rhs: Capability) -> Capability {
        Capability(self.0 | rhs.0)
    }
}

/// Bitwise-AND for `has`-style checks and set intersection.
impl std::ops::BitAnd for Capability {
    type Output = Capability;
    fn bitand(self, rhs: Capability) -> Capability {
        Capability(self.0 & rhs.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_renders_none() {
        assert_eq!(Capability(0).render(), "none");
    }

    #[test]
    fn single_bits_render_in_bit_order() {
        assert_eq!(Capability::TEXT.render(), "text");
        assert_eq!(
            (Capability::TEXT | Capability::THREAD_REPLY).render(),
            "text|thread_reply"
        );
    }

    #[test]
    fn unknown_high_bits_render_as_hex_remainder() {
        let c = Capability(1 << 20);
        assert_eq!(c.render(), "0x100000");
        // Named bits plus an unknown remainder keep both segments.
        let mixed = Capability::RICH_CARD | Capability(1 << 9);
        assert_eq!(mixed.render(), "rich_card|0x200");
    }

    #[test]
    fn has_is_includes_all_not_any() {
        let both = Capability::TEXT | Capability::VOICE;
        assert!(both.has(Capability::TEXT));
        assert!(both.has(both));
        assert!(both.has(Capability(0)));
        assert!(!both.has(Capability::VOICE | Capability::RICH_CARD));
        assert!(!Capability::TEXT.has(both));
    }

    #[test]
    fn bit_layout_matches_go_iota_order() {
        assert_eq!(Capability::TEXT.0, 1);
        assert_eq!(Capability::RICH_CARD.0, 2);
        assert_eq!(Capability::THREAD_REPLY.0, 4);
        assert_eq!(Capability::QUOTE_REPLY.0, 8);
        assert_eq!(Capability::ATTACHMENT.0, 16);
        assert_eq!(Capability::VOICE.0, 32);
        assert_eq!(Capability::TYPING_INDICATOR.0, 64);
        assert_eq!(Capability::MESSAGE_EDIT.0, 128);
    }
}
