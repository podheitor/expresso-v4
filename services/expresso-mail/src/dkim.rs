//! DKIM signing (outbound) + SPF/DKIM/DMARC verification (inbound)

use mail_auth::{
    AuthenticatedMessage,
    DkimResult,
    MessageAuthenticator,
    common::{
        crypto::{RsaKey, Sha256},
        headers::HeaderWriter,
    },
    dkim::DkimSigner,
    dmarc::verify::DmarcParameters,
    spf::verify::SpfParameters,
};
use rustls_pki_types::{PrivateKeyDer, PrivatePkcs1KeyDer, pem::PemObject};
use std::net::IpAddr;
use tracing::{debug, info, warn};

/// DKIM signer state — stores PEM bytes + config for per-message signing
pub struct DkimSignerState {
    domain: String,
    selector: String,
    pem_bytes: Vec<u8>,
}

impl DkimSignerState {
    /// Load RSA private key PEM from file, validate it can parse
    pub fn from_pem_file(domain: &str, selector: &str, pem_path: &str) -> anyhow::Result<Self> {
        let pem_bytes = std::fs::read(pem_path)?;
        // Validate key parses correctly
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

    /// Sign raw message bytes → DKIM-Signature header string
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

/// Inbound auth results — SPF + DKIM + DMARC
#[derive(Debug, Default)]
pub struct AuthResults {
    pub spf: String,
    pub dkim: String,
    pub dmarc: String,
}

impl AuthResults {
    /// Format as Authentication-Results header
    pub fn to_header(&self, hostname: &str) -> String {
        format!(
            "Authentication-Results: {}; spf={}; dkim={}; dmarc={}\r\n",
            hostname, self.spf, self.dkim, self.dmarc
        )
    }
}

/// Verify SPF + DKIM + DMARC for inbound message
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
            return AuthResults {
                spf: "temperror".into(),
                dkim: "temperror".into(),
                dmarc: "temperror".into(),
            };
        }
    };

    // SPF
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

    // DKIM + DMARC (need parsed message)
    let (dkim_str, dmarc_str) = match AuthenticatedMessage::parse(raw) {
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
            let dm = format!("{:?}", dmarc_output.dkim_result()).to_ascii_lowercase();

            (dk, dm)
        }
        None => ("permerror".to_string(), "permerror".to_string()),
    };
    debug!(dkim = %dkim_str, dmarc = %dmarc_str, "DKIM/DMARC results");

    AuthResults {
        spf: spf_str,
        dkim: dkim_str,
        dmarc: dmarc_str,
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
        };
        let hdr = ar.to_header("mx.example.com");
        assert!(hdr.starts_with("Authentication-Results: mx.example.com"));
        assert!(hdr.contains("spf=pass"));
        assert!(hdr.contains("dkim=pass"));
        assert!(hdr.contains("dmarc=pass"));
    }
}
