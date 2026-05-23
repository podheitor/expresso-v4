//! Free/busy availability queries.
//!
//! GET /api/v1/calendar/availability?user_ids=...&start=...&end=...

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_USER_IDS_PER_QUERY: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BusyStatus {
    Free,
    Busy,
    Tentative,
    OutOfOffice,
}

impl BusyStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Free        => "free",
            Self::Busy        => "busy",
            Self::Tentative   => "tentative",
            Self::OutOfOffice => "out_of_office",
        }
    }
}

impl std::fmt::Display for BusyStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusySlot {
    pub start:  String,
    pub end:    String,
    pub status: BusyStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserAvailability {
    pub user_id: Uuid,
    pub slots:   Vec<BusySlot>,
}

#[derive(Debug, Deserialize)]
pub struct AvailabilityQuery {
    pub start:    String,
    pub end:      String,
    #[serde(default)]
    pub user_ids: Vec<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_user_ids_constant() {
        assert_eq!(MAX_USER_IDS_PER_QUERY, 50);
    }

    #[test]
    fn busy_status_free_as_str() {
        assert_eq!(BusyStatus::Free.as_str(), "free");
    }

    #[test]
    fn busy_status_busy_as_str() {
        assert_eq!(BusyStatus::Busy.as_str(), "busy");
    }

    #[test]
    fn busy_status_tentative_as_str() {
        assert_eq!(BusyStatus::Tentative.as_str(), "tentative");
    }

    #[test]
    fn busy_status_out_of_office_as_str() {
        assert_eq!(BusyStatus::OutOfOffice.as_str(), "out_of_office");
    }

    #[test]
    fn busy_status_display_free() {
        assert_eq!(format!("{}", BusyStatus::Free), "free");
    }

    #[test]
    fn busy_status_display_busy() {
        assert_eq!(format!("{}", BusyStatus::Busy), "busy");
    }

    #[test]
    fn busy_status_equality() {
        assert_eq!(BusyStatus::Busy, BusyStatus::Busy);
        assert_ne!(BusyStatus::Busy, BusyStatus::Free);
    }

    #[test]
    fn busy_status_serde_roundtrip_free() {
        let s = serde_json::to_string(&BusyStatus::Free).unwrap();
        let back: BusyStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, BusyStatus::Free);
    }

    #[test]
    fn busy_status_serde_roundtrip_busy() {
        let s = serde_json::to_string(&BusyStatus::Busy).unwrap();
        let back: BusyStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, BusyStatus::Busy);
    }

    #[test]
    fn busy_slot_serde_roundtrip() {
        let slot = BusySlot {
            start: "2026-05-22T09:00:00Z".into(),
            end:   "2026-05-22T10:00:00Z".into(),
            status: BusyStatus::Busy,
        };
        let s = serde_json::to_string(&slot).unwrap();
        let back: BusySlot = serde_json::from_str(&s).unwrap();
        assert_eq!(back.start, "2026-05-22T09:00:00Z");
        assert_eq!(back.status, BusyStatus::Busy);
    }

    #[test]
    fn user_availability_serde_roundtrip() {
        let ua = UserAvailability {
            user_id: Uuid::nil(),
            slots:   vec![BusySlot {
                start: "2026-01-01T08:00:00Z".into(),
                end:   "2026-01-01T09:00:00Z".into(),
                status: BusyStatus::Tentative,
            }],
        };
        let s = serde_json::to_string(&ua).unwrap();
        let back: UserAvailability = serde_json::from_str(&s).unwrap();
        assert_eq!(back.slots.len(), 1);
    }

    #[test]
    fn busy_status_copy_trait() {
        let s = BusyStatus::Busy;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn user_availability_empty_slots() {
        let ua = UserAvailability { user_id: Uuid::nil(), slots: vec![] };
        assert!(ua.slots.is_empty());
    }

    #[test]
    fn busy_slot_status_out_of_office_roundtrip() {
        let slot = BusySlot {
            start: "T1".into(), end: "T2".into(), status: BusyStatus::OutOfOffice,
        };
        let s = serde_json::to_string(&slot).unwrap();
        let back: BusySlot = serde_json::from_str(&s).unwrap();
        assert_eq!(back.status, BusyStatus::OutOfOffice);
    }

    #[test]
    fn max_user_ids_is_positive() {
        assert!(MAX_USER_IDS_PER_QUERY > 0);
    }

    #[test]
    fn busy_status_debug_contains_variant() {
        let s = format!("{:?}", BusyStatus::Tentative);
        assert!(s.contains("Tentative"));
    }

    #[test]
    fn availability_query_default_user_ids_empty() {
        let q: AvailabilityQuery =
            serde_json::from_str(r#"{"start":"T1","end":"T2"}"#).unwrap();
        assert!(q.user_ids.is_empty());
    }

    #[test]
    fn availability_query_start_end_preserved() {
        let q: AvailabilityQuery =
            serde_json::from_str(r#"{"start":"2026-01-01T00:00:00Z","end":"2026-01-02T00:00:00Z"}"#)
                .unwrap();
        assert_eq!(q.start, "2026-01-01T00:00:00Z");
        assert_eq!(q.end, "2026-01-02T00:00:00Z");
    }

    #[test]
    fn busy_slot_clone_preserves_status() {
        let slot = BusySlot { start: "A".into(), end: "B".into(), status: BusyStatus::Free };
        assert_eq!(slot.clone().status, BusyStatus::Free);
    }

    #[test]
    fn user_availability_clone_preserves_slots_count() {
        let ua = UserAvailability {
            user_id: Uuid::nil(),
            slots: vec![
                BusySlot { start: "A".into(), end: "B".into(), status: BusyStatus::Busy },
                BusySlot { start: "C".into(), end: "D".into(), status: BusyStatus::Free },
            ],
        };
        assert_eq!(ua.clone().slots.len(), 2);
    }

    #[test]
    fn busy_status_serde_roundtrip_tentative() {
        let s = serde_json::to_string(&BusyStatus::Tentative).unwrap();
        let back: BusyStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, BusyStatus::Tentative);
    }

    #[test]
    fn busy_status_serde_roundtrip_out_of_office() {
        let s = serde_json::to_string(&BusyStatus::OutOfOffice).unwrap();
        let back: BusyStatus = serde_json::from_str(&s).unwrap();
        assert_eq!(back, BusyStatus::OutOfOffice);
    }

    #[test]
    fn busy_slot_start_preserved() {
        let slot = BusySlot {
            start: "2026-01-01T08:00:00Z".into(),
            end: "2026-01-01T09:00:00Z".into(),
            status: BusyStatus::Busy,
        };
        assert_eq!(slot.start, "2026-01-01T08:00:00Z");
    }
}
