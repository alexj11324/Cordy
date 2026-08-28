//! Temporary emergency denylist.
//!
//! Remove this once account suspension is persisted and enforced from the
//! user model.

pub const TEMPORARILY_DISABLED_USER_ERROR: &str = "account disabled";

const DISABLED_USER_IDS: [&str; 2] = [
    "514492f7-b30f-4147-bd33-c0e8ce5d6d4f",
    "1d542296-17c6-484a-9914-dcee589be116",
];

const DISABLED_USER_EMAILS: [&str; 2] = ["pdzzer68@embassybase.com", "gtwtrox@mowan666.com"];

pub fn is_temporarily_disabled_user(user_id: &str, email: &str) -> bool {
    is_temporarily_disabled_user_id(user_id) || is_temporarily_disabled_user_email(email)
}

pub fn is_temporarily_disabled_user_id(user_id: &str) -> bool {
    let id = user_id.trim().to_lowercase();
    !id.is_empty() && DISABLED_USER_IDS.contains(&id.as_str())
}

pub fn is_temporarily_disabled_user_email(email: &str) -> bool {
    let mail = email.trim().to_lowercase();
    !mail.is_empty() && DISABLED_USER_EMAILS.contains(&mail.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeded_ids_match_case_insensitively() {
        assert!(is_temporarily_disabled_user_id(
            "514492f7-b30f-4147-bd33-c0e8ce5d6d4f"
        ));
        assert!(is_temporarily_disabled_user_id(
            "  1D542296-17C6-484A-9914-DCEE589BE116  "
        ));
        assert!(!is_temporarily_disabled_user_id(
            "00000000-0000-0000-0000-000000000000"
        ));
        assert!(!is_temporarily_disabled_user_id(""));
        assert!(!is_temporarily_disabled_user_id("   "));
    }

    #[test]
    fn seeded_emails_match() {
        assert!(is_temporarily_disabled_user_email(
            "pdzzer68@embassybase.com"
        ));
        assert!(is_temporarily_disabled_user_email("GTWTROX@mowan666.com"));
        assert!(!is_temporarily_disabled_user_email("someone@example.com"));
        assert!(!is_temporarily_disabled_user_email(""));
    }

    #[test]
    fn combined_check_or_semantics() {
        assert!(is_temporarily_disabled_user(
            "514492f7-b30f-4147-bd33-c0e8ce5d6d4f",
            "someone@example.com"
        ));
        assert!(is_temporarily_disabled_user(
            "00000000-0000-0000-0000-000000000000",
            "gtwtrox@mowan666.com"
        ));
        assert!(!is_temporarily_disabled_user(
            "00000000-0000-0000-0000-000000000000",
            "someone@example.com"
        ));
    }
}
