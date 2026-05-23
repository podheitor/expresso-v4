//! Chat rooms (channels) — alias module and room-name validation helpers.

/// Maximum allowed length for a room/channel display name.
pub const MAX_ROOM_NAME_LEN: usize = 128;
/// Minimum allowed length for a room/channel display name.
pub const MIN_ROOM_NAME_LEN: usize = 1;
/// Maximum allowed length for a room topic.
pub const MAX_TOPIC_LEN: usize = 512;

/// Returns `Ok(())` when `name` is a valid room display name, otherwise
/// `Err` with a descriptive message.
pub fn validate_room_name(name: &str) -> Result<(), &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("room name must not be empty");
    }
    if trimmed.len() > MAX_ROOM_NAME_LEN {
        return Err("room name too long");
    }
    Ok(())
}

/// Returns `Ok(())` when `topic` is a valid room topic string.
pub fn validate_topic(topic: &str) -> Result<(), &'static str> {
    if topic.len() > MAX_TOPIC_LEN {
        return Err("topic too long");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_room_name_len_value() {
        assert_eq!(MAX_ROOM_NAME_LEN, 128);
    }

    #[test]
    fn min_room_name_len_value() {
        assert_eq!(MIN_ROOM_NAME_LEN, 1);
    }

    #[test]
    fn max_topic_len_value() {
        assert_eq!(MAX_TOPIC_LEN, 512);
    }

    #[test]
    fn validate_room_name_accepts_single_char() {
        assert!(validate_room_name("a").is_ok());
    }

    #[test]
    fn validate_room_name_accepts_normal_name() {
        assert!(validate_room_name("General").is_ok());
    }

    #[test]
    fn validate_room_name_rejects_empty() {
        assert!(validate_room_name("").is_err());
    }

    #[test]
    fn validate_room_name_rejects_whitespace_only() {
        assert!(validate_room_name("   ").is_err());
    }

    #[test]
    fn validate_room_name_rejects_too_long() {
        let name = "x".repeat(MAX_ROOM_NAME_LEN + 1);
        assert!(validate_room_name(&name).is_err());
    }

    #[test]
    fn validate_room_name_accepts_max_len() {
        let name = "a".repeat(MAX_ROOM_NAME_LEN);
        assert!(validate_room_name(&name).is_ok());
    }

    #[test]
    fn validate_topic_accepts_empty() {
        assert!(validate_topic("").is_ok());
    }

    #[test]
    fn validate_topic_accepts_normal_topic() {
        assert!(validate_topic("Discussion about project X").is_ok());
    }

    #[test]
    fn validate_topic_rejects_too_long() {
        let topic = "t".repeat(MAX_TOPIC_LEN + 1);
        assert!(validate_topic(&topic).is_err());
    }

    #[test]
    fn validate_topic_accepts_max_len() {
        let topic = "t".repeat(MAX_TOPIC_LEN);
        assert!(validate_topic(&topic).is_ok());
    }

    #[test]
    fn validate_room_name_error_message_empty() {
        let err = validate_room_name("").unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_room_name_error_message_too_long() {
        let name = "x".repeat(MAX_ROOM_NAME_LEN + 1);
        let err = validate_room_name(&name).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn validate_topic_error_message_too_long() {
        let topic = "t".repeat(MAX_TOPIC_LEN + 1);
        let err = validate_topic(&topic).unwrap_err();
        assert!(!err.is_empty());
    }

    #[test]
    fn min_room_name_is_less_than_max() {
        assert!(MIN_ROOM_NAME_LEN < MAX_ROOM_NAME_LEN);
    }

    #[test]
    fn max_topic_len_greater_than_max_room_name_len() {
        assert!(MAX_TOPIC_LEN > MAX_ROOM_NAME_LEN);
    }

    #[test]
    fn validate_room_name_unicode_accepted() {
        assert!(validate_room_name("Sala de Reunião").is_ok());
    }

    #[test]
    fn validate_room_name_trimmed_empty_rejected() {
        assert!(validate_room_name("  \t  ").is_err());
    }

    #[test]
    fn max_room_name_len_is_positive() {
        assert!(MAX_ROOM_NAME_LEN > 0);
    }

    #[test]
    fn max_topic_len_is_positive() {
        assert!(MAX_TOPIC_LEN > 0);
    }

    #[test]
    fn validate_topic_one_char_accepted() {
        assert!(validate_topic("x").is_ok());
    }

    #[test]
    fn validate_room_name_numbers_accepted() {
        assert!(validate_room_name("Room42").is_ok());
    }

    #[test]
    fn validate_topic_max_len_minus_one_accepted() {
        let topic = "t".repeat(MAX_TOPIC_LEN - 1);
        assert!(validate_topic(&topic).is_ok());
    }
}
