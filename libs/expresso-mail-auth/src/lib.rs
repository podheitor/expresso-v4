//! Shared SPF/DKIM/DMARC verification + DKIM signing.
//! Used by:
//!   - `expresso-mail` (ESMTP :25 native listener)
//!   - `expresso-milter` (Postfix milter sidecar)

use mail_auth::{
    AuthenticatedMessage,
    DkimResult,
    DmarcResult,
    MessageAuthenticator,
    common::{
        crypto::{RsaKey, Sha256},
        headers::HeaderWriter,
    },
    dkim::DkimSigner,
    dmarc::{Policy, verify::DmarcParameters},
    spf::verify::SpfParameters,
};
use once_cell::sync::Lazy;
use prometheus::IntCounterVec;
use rustls_pki_types::{PrivateKeyDer, PrivatePkcs1KeyDer, pem::PemObject};
use std::net::IpAddr;
use tracing::{debug, info, warn};

// Prometheus counter: result of each inbound auth check.
// Labels: check ∈ {spf,dkim,dmarc}, result ∈ {pass,fail,none,temperror,permerror}
pub static MAIL_AUTH_CHECKS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_auth_checks_total",
            "Inbound mail authentication check results",
        ),
        &["check", "result"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

// ─── DKIM SIGNING ─────────────────────────────────────────────────────────────

/// DKIM signer state — PEM bytes + config; re-parses per signing to stay Send+Sync.
pub struct DkimSignerState {
    domain: String,
    selector: String,
    pem_bytes: Vec<u8>,
}

impl DkimSignerState {
    /// Load RSA private key PEM from file; validate it parses.
    pub fn from_pem_file(domain: &str, selector: &str, pem_path: &str) -> anyhow::Result<Self> {
        let pem_bytes = std::fs::read(pem_path)?;
        let pkcs1 = PrivatePkcs1KeyDer::from_pem_slice(&pem_bytes)
            .map_err(|e| anyhow::anyhow!("invalid DKIM PEM: {e}"))?;
        let _key = RsaKey::<Sha256>::from_key_der(PrivateKeyDer::Pkcs1(pkcs1))
            .map_err(|e| anyhow::anyhow!("invalid RSA key: {e}"))?;
        info!(domain, selector, "DKIM signer loaded");
        Ok(Self {
            domain: domain.to_owned(),
            selector: selector.to_owned(),
            pem_bytes,
        })
    }

    /// Sign raw message bytes → DKIM-Signature header string.
    pub fn sign(&self, raw: &[u8]) -> anyhow::Result<String> {
        let pkcs1 = PrivatePkcs1KeyDer::from_pem_slice(&self.pem_bytes)
            .map_err(|e| anyhow::anyhow!("DKIM PEM re-parse: {e}"))?;
        let key = RsaKey::<Sha256>::from_key_der(PrivateKeyDer::Pkcs1(pkcs1))
            .map_err(|e| anyhow::anyhow!("DKIM key rebuild: {e}"))?;
        let sig = DkimSigner::from_key(key)
            .domain(&self.domain)
            .selector(&self.selector)
            .headers(["From", "To", "Subject", "Date", "Message-ID"])
            .sign(raw)
            .map_err(|e| anyhow::anyhow!("DKIM sign: {e}"))?;
        Ok(sig.to_header())
    }
}

// ─── INBOUND VERIFICATION ─────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct AuthResults {
    pub spf: String,
    pub dkim: String,
    pub dmarc: String,
    /// Published DMARC policy on the From domain. Values: "none", "quarantine",
    /// "reject", "unspecified" (no record) or `None` when not evaluated.
    pub dmarc_policy: Option<String>,
}

impl AuthResults {
    /// Format as full Authentication-Results header (with CRLF). Includes
    /// "(p=<policy>)" suffix on dmarc when a policy was observed.
    pub fn to_header(&self, hostname: &str) -> String {
        format!("Authentication-Results: {}\r\n", self.to_value(hostname))
    }

    /// Format just the value (no "Authentication-Results: " prefix, no CRLF)
    /// — useful when caller injects via milter `add_header(name, value)`.
    pub fn to_value(&self, hostname: &str) -> String {
        let dmarc_frag = match self.dmarc_policy.as_deref() {
            Some(p) if p != "unspecified" => format!("dmarc={} (p={})", self.dmarc, p),
            _ => format!("dmarc={}", self.dmarc),
        };
        format!(
            "{}; spf={}; dkim={}; {}",
            hostname, self.spf, self.dkim, dmarc_frag
        )
    }

    /// True when DMARC verdict is `fail` and the published policy asks for reject.
    pub fn should_reject(&self) -> bool {
        self.dmarc == "fail" && self.dmarc_policy.as_deref() == Some("reject")
    }

    /// True when DMARC verdict is `fail` and the published policy asks for quarantine.
    pub fn should_quarantine(&self) -> bool {
        self.dmarc == "fail" && self.dmarc_policy.as_deref() == Some("quarantine")
    }
}

/// Prometheus counter: policy action taken after verify.
/// Labels: action ∈ {accept, reject, quarantine}
pub static MAIL_AUTH_ACTIONS_TOTAL: Lazy<IntCounterVec> = Lazy::new(|| {
    let c = IntCounterVec::new(
        prometheus::Opts::new(
            "mail_auth_actions_total",
            "Policy action taken after inbound auth verify",
        ),
        &["action"],
    )
    .expect("metric build");
    expresso_observability::register(c)
});

fn policy_str(p: Policy) -> &'static str {
    match p {
        Policy::None        => "none",
        Policy::Quarantine  => "quarantine",
        Policy::Reject      => "reject",
        Policy::Unspecified => "unspecified",
    }
}

fn dmarc_verdict(spf: &DmarcResult, dkim: &DmarcResult) -> &'static str {
    // DMARC pass = at least one of SPF/DKIM is aligned Pass.
    if matches!(spf, DmarcResult::Pass) || matches!(dkim, DmarcResult::Pass) {
        "pass"
    } else if matches!(spf, DmarcResult::TempError(_)) || matches!(dkim, DmarcResult::TempError(_)) {
        "temperror"
    } else if matches!(spf, DmarcResult::PermError(_)) || matches!(dkim, DmarcResult::PermError(_)) {
        "permerror"
    } else if matches!(spf, DmarcResult::None) && matches!(dkim, DmarcResult::None) {
        "none"
    } else {
        "fail"
    }
}

/// Verify SPF + DKIM + DMARC for inbound message.
pub async fn verify_inbound(
    peer_ip: IpAddr,
    helo_domain: &str,
    mail_from: &str,
    hostname: &str,
    raw: &[u8],
) -> AuthResults {
    let authenticator = match MessageAuthenticator::new_cloudflare_tls() {
        Ok(a) => a,
        Err(e) => {
            warn!(error = %e, "authenticator init failed — skipping auth");
            MAIL_AUTH_CHECKS_TOTAL.with_label_values(&["spf",   "temperror"]).inc();
            MAIL_AUTH_CHECKS_TOTAL.with_label_values(&["dkim",  "temperror"]).inc();
            MAIL_AUTH_CHECKS_TOTAL.with_label_values(&["dmarc", "temperror"]).inc();
            return AuthResults {
                spf: "temperror".into(),
                dkim: "temperror".into(),
                dmarc: "temperror".into(),
                dmarc_policy: None,
            };
        }
    };

    let from_domain = mail_from.rsplit_once('@').map(|(_, d)| d).unwrap_or(helo_domain);

    let spf_output = authenticator
        .verify_spf(SpfParameters::verify_mail_from(
            peer_ip,
            from_domain,
            hostname,
            mail_from,
        ))
        .await;
    let spf_str = format!("{:?}", spf_output.result()).to_ascii_lowercase();
    debug!(spf = %spf_str, from_domain, "SPF result");

    let (dkim_str, dmarc_str, dmarc_policy) = match AuthenticatedMessage::parse(raw) {
        Some(authenticated) => {
            let dkim_results = authenticator.verify_dkim(&authenticated).await;
            let dk = if dkim_results.is_empty() {
                "none".to_string()
            } else if dkim_results.iter().all(|r| r.result() == &DkimResult::Pass) {
                "pass".to_string()
            } else {
                "fail".to_string()
            };

            let dmarc_output = authenticator
                .verify_dmarc(DmarcParameters::new(
                    &authenticated,
                    &dkim_results,
                    from_domain,
                    &spf_output,
                ))
                .await;
            let dm = dmarc_verdict(dmarc_output.spf_result(), dmarc_output.dkim_result()).to_string();
            let policy = Some(policy_str(dmarc_output.policy()).to_string());
            (dk, dm, policy)
        }
        None => ("permerror".to_string(), "permerror".to_string(), None),
    };
    debug!(dkim = %dkim_str, dmarc = %dmarc_str, policy = ?dmarc_policy, "DKIM/DMARC results");

    MAIL_AUTH_CHECKS_TOTAL.with_label_values(&["spf",   spf_str.as_str()]).inc();
    MAIL_AUTH_CHECKS_TOTAL.with_label_values(&["dkim",  dkim_str.as_str()]).inc();
    MAIL_AUTH_CHECKS_TOTAL.with_label_values(&["dmarc", dmarc_str.as_str()]).inc();

    AuthResults {
        spf: spf_str,
        dkim: dkim_str,
        dmarc: dmarc_str,
        dmarc_policy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_results_header_format() {
        let ar = AuthResults {
            spf: "pass".into(),
            dkim: "pass".into(),
            dmarc: "pass".into(),
            dmarc_policy: None,
        };
        let hdr = ar.to_header("mx.example.com");
        assert!(hdr.starts_with("Authentication-Results: mx.example.com"));
        assert!(hdr.contains("spf=pass"));
        assert!(hdr.contains("dkim=pass"));
        assert!(hdr.contains("dmarc=pass"));

        let v = ar.to_value("mx.example.com");
        assert_eq!(v, "mx.example.com; spf=pass; dkim=pass; dmarc=pass");
    }

    #[test]
    fn dmarc_policy_appended_to_header() {
        let ar = AuthResults {
            spf: "fail".into(),
            dkim: "none".into(),
            dmarc: "fail".into(),
            dmarc_policy: Some("reject".into()),
        };
        let v = ar.to_value("mx.example.com");
        assert!(v.contains("dmarc=fail (p=reject)"), "got {v}");
    }

    #[test]
    fn unspecified_policy_not_rendered() {
        let ar = AuthResults {
            spf: "pass".into(),
            dkim: "pass".into(),
            dmarc: "pass".into(),
            dmarc_policy: Some("unspecified".into()),
        };
        let v = ar.to_value("mx.example.com");
        assert!(!v.contains("(p="), "unexpected policy fragment: {v}");
    }

    #[test]
    fn should_quarantine_only_when_fail_and_p_quarantine() {
        let fail_quar = AuthResults {
            spf: "fail".into(), dkim: "fail".into(), dmarc: "fail".into(),
            dmarc_policy: Some("quarantine".into()),
        };
        assert!(fail_quar.should_quarantine());
        assert!(!fail_quar.should_reject());

        let pass_quar = AuthResults {
            spf: "pass".into(), dkim: "pass".into(), dmarc: "pass".into(),
            dmarc_policy: Some("quarantine".into()),
        };
        assert!(!pass_quar.should_quarantine());
    }

    #[test]
    fn should_reject_only_when_fail_and_p_reject() {
        let fail_reject = AuthResults {
            spf: "fail".into(), dkim: "fail".into(), dmarc: "fail".into(),
            dmarc_policy: Some("reject".into()),
        };
        assert!(fail_reject.should_reject());

        let fail_quar = AuthResults {
            spf: "fail".into(), dkim: "fail".into(), dmarc: "fail".into(),
            dmarc_policy: Some("quarantine".into()),
        };
        assert!(!fail_quar.should_reject());

        let pass = AuthResults {
            spf: "pass".into(), dkim: "pass".into(), dmarc: "pass".into(),
            dmarc_policy: Some("reject".into()),
        };
        assert!(!pass.should_reject());
    }
}
