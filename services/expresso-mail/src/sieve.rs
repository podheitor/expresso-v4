//! Sieve filter engine — evaluates user scripts on inbound mail.
//! Supports: fileinto, reject, discard, keep, redirect (RFC 5228 subset).

use sieve::{Compiler, Event, Input, Recipient, Runtime};
use tracing::{debug, warn};

/// Result of running a sieve filter on a message
#[derive(Debug, Clone)]
pub enum FilterAction {
    /// Deliver to default inbox
    Keep { flags: Vec<String> },
    /// Deliver to specified folder
    FileInto { folder: String, flags: Vec<String> },
    /// Reject with reason (bounce to sender)
    Reject { reason: String },
    /// Silently discard
    Discard,
    /// Redirect to another address
    Redirect { address: String },
}

/// Extract address string from sieve Recipient enum
fn recipient_address(r: &Recipient) -> String {
    match r {
        Recipient::Address(a) => a.clone(),
        Recipient::List(l) => l.clone(),
        Recipient::Group(g) => g.join(", "),
    }
}

/// Run a sieve script against raw message bytes.
/// Returns ordered list of actions to execute.
pub fn evaluate(script_src: &[u8], raw_message: &[u8]) -> Vec<FilterAction> {
    let compiler = Compiler::new();
    let script = match compiler.compile(script_src) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "sieve compile error — defaulting to Keep");
            return vec![FilterAction::Keep { flags: vec![] }];
        }
    };

    let runtime = Runtime::new();
    let mut instance = runtime.filter(raw_message);
    let mut input = Input::script("user-filter", script);
    let mut actions = Vec::new();

    while let Some(result) = instance.run(input) {
        match result {
            Ok(event) => {
                input = match event {
                    Event::Keep { flags, .. } => {
                        let flags: Vec<String> = flags.into_iter().map(|f| f.to_string()).collect();
                        debug!(?flags, "sieve: keep");
                        actions.push(FilterAction::Keep { flags });
                        true.into()
                    }
                    Event::FileInto { folder, flags, .. } => {
                        let flags: Vec<String> = flags.into_iter().map(|f| f.to_string()).collect();
                        debug!(folder = %folder, ?flags, "sieve: fileinto");
                        actions.push(FilterAction::FileInto {
                            folder: folder.to_string(),
                            flags,
                        });
                        true.into()
                    }
                    Event::Reject { reason, .. } => {
                        debug!(reason = %reason, "sieve: reject");
                        actions.push(FilterAction::Reject {
                            reason: reason.to_string(),
                        });
                        true.into()
                    }
                    Event::Discard => {
                        debug!("sieve: discard");
                        actions.push(FilterAction::Discard);
                        true.into()
                    }
                    Event::SendMessage { recipient, .. } => {
                        let addr = recipient_address(&recipient);
                        debug!(to = %addr, "sieve: redirect");
                        actions.push(FilterAction::Redirect { address: addr });
                        true.into()
                    }
                    Event::MailboxExists { .. }
                    | Event::ListContains { .. }
                    | Event::DuplicateId { .. } => false.into(),
                    Event::SetEnvelope { .. } => true.into(),
                    Event::IncludeScript { optional, .. } => {
                        if optional {
                            Input::False
                        } else {
                            warn!("sieve: include not supported");
                            Input::False
                        }
                    }
                    Event::CreatedMessage { .. } => true.into(),
                    Event::Notify { .. } => true.into(),
                    Event::Function { .. } => Input::result("".into()),
                };
            }
            Err(e) => {
                warn!(error = ?e, "sieve runtime error");
                input = true.into();
            }
        }
    }

    if actions.is_empty() {
        actions.push(FilterAction::Keep { flags: vec![] });
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    const MSG: &str = "From: alice@example.com\r\nTo: bob@example.com\r\nSubject: TPS Report\r\n\r\nPlease review the TPS report.\r\n";

    #[test]
    fn sieve_fileinto() {
        let script = br#"
            require "fileinto";
            if header :contains "Subject" "TPS" {
                fileinto "Work";
            }
        "#;
        let actions = evaluate(script, MSG.as_bytes());
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            FilterAction::FileInto { folder, .. } => assert_eq!(folder, "Work"),
            other => panic!("expected FileInto, got {other:?}"),
        }
    }

    #[test]
    fn sieve_reject() {
        let script = br#"
            require "reject";
            reject "No thanks";
        "#;
        let actions = evaluate(script, MSG.as_bytes());
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            FilterAction::Reject { reason } => assert_eq!(reason, "No thanks"),
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn sieve_discard() {
        let script = b"discard;";
        let actions = evaluate(script, MSG.as_bytes());
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], FilterAction::Discard));
    }

    #[test]
    fn sieve_default_keep() {
        let script = b"";
        let actions = evaluate(script, MSG.as_bytes());
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], FilterAction::Keep { .. }));
    }
}
