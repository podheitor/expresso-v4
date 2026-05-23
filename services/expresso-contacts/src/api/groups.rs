//! Contact groups (distribution lists) — pure-logic types and validation.

/// Maximum number of members allowed in a single contact group.
pub const MAX_GROUP_MEMBERS: usize = 5_000;
/// Maximum length for a group display name.
pub const MAX_GROUP_NAME_LEN: usize = 256;
/// Minimum length for a group display name.
pub const MIN_GROUP_NAME_LEN: usize = 1;

/// Returns `Ok(())` when `name` is a valid group name.
pub fn validate_group_name(name: &str) -> Result<(), &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("group name must not be empty");
    }
    if trimmed.len() > MAX_GROUP_NAME_LEN {
        return Err("group name too long");
    }
    Ok(())
}

/// Returns `Ok(())` when `count` does not exceed the member cap.
pub fn validate_member_count(count: usize) -> Result<(), &'static str> {
    if count > MAX_GROUP_MEMBERS {
        return Err("group has too many members");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_group_members_value() {
        assert_eq!(MAX_GROUP_MEMBERS, 5_000);
    }

    #[test]
    fn max_group_name_len_value() {
        assert_eq!(MAX_GROUP_NAME_LEN, 256);
    }

    #[test]
    fn min_group_name_len_value() {
        assert_eq!(MIN_GROUP_NAME_LEN, 1);
    }

    #[test]
    fn validate_group_name_accepts_single_char() {
        assert!(validate_group_name("A").is_ok());
    }

    #[test]
    fn validate_group_name_accepts_normal_name() {
        assert!(validate_group_name("Marketing Team").is_ok());
    }

    #[test]
    fn validate_group_name_rejects_empty() {
        assert!(validate_group_name("").is_err());
    }

    #[test]
    fn validate_group_name_rejects_whitespace_only() {
        assert!(validate_group_name("   ").is_err());
    }

    #[test]
    fn validate_group_name_rejects_too_long() {
        let name = "a".repeat(MAX_GROUP_NAME_LEN + 1);
        assert!(validate_group_name(&name).is_err());
    }

    #[test]
    fn validate_group_name_accepts_max_len() {
        let name = "a".repeat(MAX_GROUP_NAME_LEN);
        assert!(validate_group_name(&name).is_ok());
    }

    #[test]
    fn validate_member_count_accepts_zero() {
        assert!(validate_member_count(0).is_ok());
    }

    #[test]
    fn validate_member_count_accepts_max() {
        assert!(validate_member_count(MAX_GROUP_MEMBERS).is_ok());
    }

    #[test]
    fn validate_member_count_rejects_over_max() {
        assert!(validate_member_count(MAX_GROUP_MEMBERS + 1).is_err());
    }

    #[test]
    fn validate_member_count_accepts_one() {
        assert!(validate_member_count(1).is_ok());
    }

    #[test]
    fn validate_group_name_error_on_empty_is_nonempty_msg() {
        let err = validate_group_name("").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_group_name_error_on_too_long_is_nonempty_msg() {
        let name = "x".repeat(MAX_GROUP_NAME_LEN + 1);
        let err = validate_group_name(&name).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_member_count_error_msg_is_nonempty() {
        let err = validate_member_count(MAX_GROUP_MEMBERS + 1).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn min_is_less_than_max_name_len() {
        assert!(MIN_GROUP_NAME_LEN < MAX_GROUP_NAME_LEN);
    }

    #[test]
    fn max_group_members_is_positive() {
        assert!(MAX_GROUP_MEMBERS > 0);
    }

    #[test]
    fn max_group_name_len_is_positive() {
        assert!(MAX_GROUP_NAME_LEN > 0);
    }

    #[test]
    fn validate_group_name_unicode_accepted() {
        assert!(validate_group_name("Equipe de Vendas").is_ok());
    }

    #[test]
    fn validate_member_count_large_value_over_cap_rejected() {
        assert!(validate_member_count(100_000).is_err());
    }

    #[test]
    fn validate_group_name_tab_only_rejected() {
        assert!(validate_group_name("\t\t").is_err());
    }

    #[test]
    fn max_group_members_greater_than_max_group_name_len() {
        assert!(MAX_GROUP_MEMBERS > MAX_GROUP_NAME_LEN);
    }

    #[test]
    fn validate_member_count_max_minus_one_accepted() {
        assert!(validate_member_count(MAX_GROUP_MEMBERS - 1).is_ok());
    }
}
