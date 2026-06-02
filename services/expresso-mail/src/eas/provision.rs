//! EAS Provision command (MS-ASPROV).
//!
//! Provision hands the device its security policy before it may sync. The policy
//! values come from config (`mail_server.activesync_*`): whether a PIN is
//! required, the minimum length, and the failed-attempt count that triggers a
//! local wipe. The response is Status 1 + a Policy of type
//! `MS-EAS-Provisioning-WBXML` carrying an EASProvisionDoc with those settings,
//! plus a PolicyKey the client echoes on subsequent commands.

use expresso_wbxml::{
    encode,
    tokens::{page, provision},
    Event,
};

/// The fixed policy key handed to every device (no per-device policy state in
/// the MVP). A non-zero numeric string is all a client requires.
pub const POLICY_KEY: &str = "1";

/// Device-policy values to advertise, resolved from config.
#[derive(Debug, Clone, Copy)]
pub struct DevicePolicy {
    pub require_pin: bool,
    pub min_pin_len: u32,
    pub max_pin_failures: u32,
}

/// Build the WBXML Provision response carrying `policy`.
pub fn provision_response(policy: DevicePolicy) -> Vec<u8> {
    let p = page::PROVISION;
    let mut doc = vec![
        Event::start(p, provision::PROVISION),
        Event::start(p, provision::STATUS),
        Event::Text("1".into()),
        Event::EndElement,
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
        // Data > EASProvisionDoc with the actual settings.
        Event::start(p, provision::DATA),
        Event::start(p, provision::EAS_PROVISION_DOC),
    ];
    push(
        &mut doc,
        provision::DEVICE_PASSWORD_ENABLED,
        bool01(policy.require_pin),
    );
    if policy.require_pin {
        // Allow simple (numeric) PINs; require non-alphanumeric only when asked.
        push(&mut doc, provision::ALLOW_SIMPLE_DEVICE_PASSWORD, "1");
        push(&mut doc, provision::ALPHANUMERIC_PWD_REQUIRED, "0");
        push(
            &mut doc,
            provision::MIN_DEVICE_PASSWORD_LENGTH,
            &policy.min_pin_len.to_string(),
        );
        push(
            &mut doc,
            provision::MAX_PASSWORD_FAILED_ATTEMPTS,
            &policy.max_pin_failures.to_string(),
        );
    }
    doc.push(Event::EndElement); // EASProvisionDoc
    doc.push(Event::EndElement); // Data
    doc.push(Event::EndElement); // Policy
    doc.push(Event::EndElement); // Policies
    doc.push(Event::EndElement); // Provision
    encode(&doc)
}

fn push(doc: &mut Vec<Event>, token: u8, text: &str) {
    doc.push(Event::start(page::PROVISION, token));
    doc.push(Event::Text(text.into()));
    doc.push(Event::EndElement);
}

fn bool01(v: bool) -> &'static str {
    if v {
        "1"
    } else {
        "0"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expresso_wbxml::decode;

    fn policy() -> DevicePolicy {
        DevicePolicy {
            require_pin: true,
            min_pin_len: 4,
            max_pin_failures: 8,
        }
    }

    #[test]
    fn provision_response_round_trips() {
        let bytes = provision_response(policy());
        let events = decode(&bytes).unwrap();
        assert_eq!(encode(&events), bytes);
    }

    #[test]
    fn provision_response_carries_policy_key() {
        let events = decode(&provision_response(policy())).unwrap();
        assert!(events
            .iter()
            .any(|e| matches!(e, Event::Text(t) if t == POLICY_KEY)));
    }

    #[test]
    fn provision_response_emits_pin_settings_when_required() {
        let events = decode(&provision_response(policy())).unwrap();
        let texts: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        // min length 4, max failures 8 present.
        assert!(texts.contains(&"4"));
        assert!(texts.contains(&"8"));
        // DevicePasswordEnabled element present.
        assert!(events.iter().any(|e| matches!(
            e,
            Event::StartElement { token, .. } if *token == provision::DEVICE_PASSWORD_ENABLED
        )));
    }

    #[test]
    fn provision_response_no_pin_settings_when_permissive() {
        let p = DevicePolicy {
            require_pin: false,
            min_pin_len: 4,
            max_pin_failures: 8,
        };
        let events = decode(&provision_response(p)).unwrap();
        // No MinDevicePasswordLength element when PIN not required.
        assert!(!events.iter().any(|e| matches!(
            e,
            Event::StartElement { token, .. } if *token == provision::MIN_DEVICE_PASSWORD_LENGTH
        )));
    }

    #[test]
    fn provision_response_starts_on_provision_page() {
        let events = decode(&provision_response(policy())).unwrap();
        assert_eq!(
            events[0],
            Event::start(page::PROVISION, provision::PROVISION)
        );
    }
}
