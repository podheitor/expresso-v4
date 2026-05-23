//! Calendar event invitations (iTIP REPLY handling).
//!
//! POST /api/v1/calendar/events/:uid/invite  — send invite to attendees
//! POST /api/v1/calendar/events/:uid/rsvp    — RSVP (accept / decline / tentative)

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_INVITEE_COUNT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RsvpStatus {
    Accepted,
    Declined,
    Tentative,
    NeedsAction,
}

impl RsvpStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted    => "accepted",
            Self::Declined    => "declined",
            Self::Tentative   => "tentative",
            Self::NeedsAction => "needs-action",
        }
    }
}

impl std::fmt::Display for RsvpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attendee {
    pub user_id: Uuid,
    pub email:   String,
    pub status:  RsvpStatus,
}

#[derive(Debug, Deserialize)]
pub struct InviteBody {
    pub attendee_ids: Vec<Uuid>,
    pub message:      Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RsvpBody {
    pub status:  RsvpStatus,
    pub comment: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_invitee_count_constant() {
        assert_eq!(MAX_INVITEE_COUNT, 200);
    }

    #[test]
    fn rsvp_accepted_as_str() {
        assert_eq!(RsvpStatus::Accepted.as_str(), "accepted");
    }

    #[test]
    fn rsvp_declined_as_str() {
        assert_eq!(RsvpStatus::Declined.as_str(), "declined");
    }

    #[test]
    fn rsvp_tentative_as_str() {
        assert_eq!(RsvpStatus::Tentative.as_str(), "tentative");
    }

    #[test]
    fn rsvp_needs_action_as_str() {
        assert_eq!(RsvpStatus::NeedsAction.as_str(), "needs-action");
    }

    #[test]
    fn rsvp_display_accepted() {
        assert_eq!(format!("{}", RsvpStatus::Accepted), "accepted");
    }

    #[test]
    fn rsvp_display_declined() {
        assert_eq!(format!("{}", RsvpStatus::Declined), "declined");
    }

    #[test]
    fn rsvp_equality() {
        assert_eq!(RsvpStatus::Accepted, RsvpStatus::Accepted);
        assert_ne!(RsvpStatus::Accepted, RsvpStatus::Declined);
    }

    #[test]
    fn rsvp_serde_roundtrip_accepted() {
        let s = serde_json::to_string(&RsvpStatus::Accepted).unwrap();
        let back: RsvpStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RsvpStatus::Accepted);
    }

    #[test]
    fn rsvp_serde_roundtrip_tentative() {
        let s = serde_json::to_string(&RsvpStatus::Tentative).unwrap();
        let back: RsvpStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RsvpStatus::Tentative);
    }

    #[test]
    fn attendee_serde_roundtrip() {
        let a = Attendee {
            user_id: Uuid::nil(),
            email:   "user@example.com".into(),
            status:  RsvpStatus::Accepted,
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: Attendee = serde_json::from_str(&s).unwrap();
        assert_eq!(back.email, "user@example.com");
        assert_eq!(back.status, RsvpStatus::Accepted);
    }

    #[test]
    fn invite_body_message_optional() {
        let b: InviteBody =
            serde_json::from_str(r#"{"attendee_ids":[]}"#).unwrap();
        assert!(b.message.is_none());
    }

    #[test]
    fn invite_body_attendees_empty_list() {
        let b: InviteBody =
            serde_json::from_str(r#"{"attendee_ids":[]}"#).unwrap();
        assert!(b.attendee_ids.is_empty());
    }

    #[test]
    fn rsvp_body_comment_optional() {
        let b: RsvpBody =
            serde_json::from_str(r#"{"status":"accepted"}"#).unwrap();
        assert!(b.comment.is_none());
    }

    #[test]
    fn rsvp_body_with_comment() {
        let b: RsvpBody =
            serde_json::from_str(r#"{"status":"declined","comment":"busy that day"}"#).unwrap();
        assert_eq!(b.comment.as_deref(), Some("busy that day"));
    }

    #[test]
    fn rsvp_copy_trait() {
        let s = RsvpStatus::NeedsAction;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn attendee_clone_preserves_email() {
        let a = Attendee {
            user_id: Uuid::nil(), email: "a@b.com".into(), status: RsvpStatus::Tentative,
        };
        assert_eq!(a.clone().email, "a@b.com");
    }

    #[test]
    fn max_invitee_count_positive() {
        assert!(MAX_INVITEE_COUNT > 0);
    }

    #[test]
    fn rsvp_debug_contains_variant() {
        let s = format!("{:?}", RsvpStatus::Declined);
        assert!(s.contains("Declined"));
    }

    #[test]
    fn rsvp_serde_roundtrip_declined() {
        let s = serde_json::to_string(&RsvpStatus::Declined).unwrap();
        let back: RsvpStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RsvpStatus::Declined);
    }

    #[test]
    fn attendee_status_field_accessible() {
        let a = Attendee {
            user_id: Uuid::nil(), email: "x@y.com".into(), status: RsvpStatus::NeedsAction,
        };
        assert_eq!(a.status, RsvpStatus::NeedsAction);
    }

    #[test]
    fn rsvp_body_status_declined_roundtrip() {
        let b: RsvpBody = serde_json::from_str(r#"{"status":"declined"}"#).unwrap();
        assert_eq!(b.status, RsvpStatus::Declined);
    }

    #[test]
    fn invite_body_message_with_text() {
        let b: InviteBody =
            serde_json::from_str(r#"{"attendee_ids":[],"message":"Please join us"}"#).unwrap();
        assert_eq!(b.message.as_deref(), Some("Please join us"));
    }

    #[test]
    fn rsvp_serde_roundtrip_needs_action() {
        let s = serde_json::to_string(&RsvpStatus::NeedsAction).unwrap();
        let back: RsvpStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, RsvpStatus::NeedsAction);
    }
}
