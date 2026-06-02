//! Minimal EAS Provision command (MS-ASPROV).
//!
//! Clients issue Provision to fetch a device policy before they may sync. We run
//! a permissive policy (no PIN, no remote-wipe enforcement) — the response just
//! grants a policy key the client echoes back on subsequent commands. Real
//! device-policy enforcement is a later, optional sprint.
//!
//! The two-phase handshake collapses to: respond with Status 1, a Policy of type
//! `MS-EAS-Provisioning-WBXML` with Status 1, and a PolicyKey. The same key is
//! returned in both phases; clients accept this for a no-op policy.

use expresso_wbxml::{
    encode,
    tokens::{page, provision},
    Event,
};

/// The fixed policy key handed to every device (no per-device policy state in
/// the MVP). A non-zero numeric string is all a permissive client requires.
pub const POLICY_KEY: &str = "1";

/// Build the WBXML Provision response body.
pub fn provision_response() -> Vec<u8> {
    let p = page::PROVISION;
    let doc = vec![
        Event::start(p, provision::PROVISION),
        // Top-level Status = 1 (success).
        Event::start(p, provision::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
        // Policies > Policy.
        Event::start(p, provision::POLICIES),
        Event::start(p, provision::POLICY),
        Event::start(p, provision::POLICY_TYPE),
        Event::Text("MS-EAS-Provisioning-WBXML".into()),
        Event::EndElement,
        Event::start(p, provision::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
        Event::start(p, provision::POLICY_KEY),
        Event::Text(POLICY_KEY.into()),
        Event::EndElement,
        Event::EndElement, // Policy
        Event::EndElement, // Policies
        Event::EndElement, // Provision
    ];
    encode(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use expresso_wbxml::decode;

    #[test]
    fn provision_response_round_trips() {
        let bytes = provision_response();
        let events = decode(&bytes).unwrap();
        // Round-trips through the codec and re-encodes identically.
        assert_eq!(encode(&events), bytes);
    }

    #[test]
    fn provision_response_carries_policy_key() {
        let events = decode(&provision_response()).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Text(t) if t == POLICY_KEY)));
    }

    #[test]
    fn provision_response_starts_on_provision_page() {
        let events = decode(&provision_response()).unwrap();
        assert_eq!(
            events[0],
            Event::start(page::PROVISION, provision::PROVISION)
        );
    }

    #[test]
    fn provision_response_is_nonempty() {
        assert!(!provision_response().is_empty());
    }
}
