//! Content scanner: Rspamd (spam) + ClamAV (malware) before delivery.
//!
//! Both are optional. If env vars not set → no-op (returns Clean).
//! Called from ingest::process before persisting message to mailbox.
//!
//! Env:
//!   MAIL__RSPAMD_URL    http://rspamd:11333  (POST /checkv2)
//!   MAIL__CLAMAV_ADDR   clamav:3310          (TCP INSTREAM)
//!   MAIL__SPAM_REJECT_SCORE  float (default 15.0) — above = reject
//!   MAIL__CLAMAV_TIMEOUT_MS  default 30000

use std::time::Duration;
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct ScanResult {
    pub spam_score: Option<f32>,
    pub spam_action: Option<String>, // "no action" | "add header" | "reject" | etc.
    pub virus: Option<String>,       // Some(name) if infected
}

impl ScanResult {
    pub fn is_clean(&self) -> bool {
        self.virus.is_none()
            && self
                .spam_action
                .as_deref()
                .map(|a| a != "reject")
                .unwrap_or(true)
    }

    /// Headers to prepend to raw message (X-Spam-*, X-Virus-*).
    pub fn to_headers(&self) -> String {
        let mut out = String::new();
        if let Some(score) = self.spam_score {
            out.push_str(&format!(
                "X-Spam-Score: {:.2}\r\nX-Spam-Status: {}\r\n",
                score,
                if score >= 5.0 { "Yes" } else { "No" }
            ));
        }
        if let Some(action) = &self.spam_action {
            out.push_str(&format!("X-Spam-Action: {}\r\n", action));
        }
        if let Some(v) = &self.virus {
            out.push_str(&format!("X-Virus-Status: Infected: {}\r\n", v));
        } else {
            out.push_str("X-Virus-Status: Clean\r\n");
        }
        out
    }
}

#[derive(Debug, Deserialize)]
struct RspamdResp {
    score: f32,
    action: String,
}

/// Scan message — calls Rspamd and ClamAV if configured.
/// Never fails hard — errors log and return partial result (fail-open for delivery).
pub async fn scan(raw: &[u8]) -> ScanResult {
    let mut r = ScanResult { spam_score: None, spam_action: None, virus: None };

    if let Ok(url) = std::env::var("MAIL__RSPAMD_URL") {
        match rspamd_scan(&url, raw).await {
            Ok(resp) => {
                r.spam_score = Some(resp.score);
                r.spam_action = Some(resp.action);
            }
            Err(e) => warn!(error = %e, "rspamd scan failed (fail-open)"),
        }
    }

    if let Ok(addr) = std::env::var("MAIL__CLAMAV_ADDR") {
        match clamav_scan(&addr, raw).await {
            Ok(Some(virus)) => r.virus = Some(virus),
            Ok(None) => {}
            Err(e) => warn!(error = %e, "clamav scan failed (fail-open)"),
        }
    }

    debug!(?r, "scan result");
    r
}

async fn rspamd_scan(url: &str, raw: &[u8]) -> anyhow::Result<RspamdResp> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;
    let endpoint = format!("{}/checkv2", url.trim_end_matches('/'));
    let resp = client
        .post(&endpoint)
        .body(raw.to_vec())
        .send()
        .await?
        .error_for_status()?
        .json::<RspamdResp>()
        .await?;
    Ok(resp)
}

/// INSTREAM protocol (clamd):
///   zINSTREAM\0  <u32 BE len> <chunk>  <u32 BE 0>
/// Response: "stream: OK" or "stream: Foo.Virus FOUND"
async fn clamav_scan(addr: &str, raw: &[u8]) -> anyhow::Result<Option<String>> {
    let timeout_ms: u64 = std::env::var("MAIL__CLAMAV_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30_000);

    let fut = async {
        let mut stream = TcpStream::connect(addr).await?;
        stream.write_all(b"zINSTREAM\0").await?;
        // Send in ≤64KiB chunks
        for chunk in raw.chunks(64 * 1024) {
            let len = (chunk.len() as u32).to_be_bytes();
            stream.write_all(&len).await?;
            stream.write_all(chunk).await?;
        }
        stream.write_all(&0u32.to_be_bytes()).await?;
        stream.flush().await?;

        let mut reply = Vec::with_capacity(128);
        stream.read_to_end(&mut reply).await?;
        let s = String::from_utf8_lossy(&reply).trim().to_string();
        Ok::<String, anyhow::Error>(s)
    };

    let reply = tokio::time::timeout(Duration::from_millis(timeout_ms), fut)
        .await
        .map_err(|_| anyhow::anyhow!("clamav timeout after {}ms", timeout_ms))??;

    debug!(%reply, "clamav raw");
    if reply.contains("FOUND") {
        // "stream: Eicar-Test-Signature FOUND"
        let name = reply
            .trim_start_matches("stream:")
            .trim()
            .trim_end_matches("FOUND")
            .trim()
            .to_string();
        Ok(Some(name))
    } else if reply.contains("OK") {
        Ok(None)
    } else {
        anyhow::bail!("unexpected clamd reply: {reply}");
    }
}

/// Decision based on score threshold + virus presence.
/// Returns Some(reason) if delivery should be REJECTED.
pub fn should_reject(r: &ScanResult) -> Option<String> {
    if let Some(v) = &r.virus {
        return Some(format!("infected: {}", v));
    }
    let reject_score: f32 = std::env::var("MAIL__SPAM_REJECT_SCORE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15.0);
    if let Some(score) = r.spam_score {
        if score >= reject_score {
            return Some(format!("spam score {:.2} >= {:.2}", score, reject_score));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_no_virus_no_spam() {
        let r = ScanResult { spam_score: Some(2.5), spam_action: Some("no action".into()), virus: None };
        assert!(r.is_clean());
        assert!(should_reject(&r).is_none());
    }

    #[test]
    fn virus_rejects() {
        let r = ScanResult { spam_score: None, spam_action: None, virus: Some("Eicar".into()) };
        assert!(!r.is_clean());
        assert_eq!(should_reject(&r).unwrap(), "infected: Eicar");
    }

    #[test]
    fn high_score_rejects() {
        let r = ScanResult { spam_score: Some(20.0), spam_action: Some("reject".into()), virus: None };
        assert!(!r.is_clean());
        assert!(should_reject(&r).unwrap().contains("spam score"));
    }

    #[test]
    fn headers_include_both() {
        let r = ScanResult { spam_score: Some(3.14), spam_action: Some("add header".into()), virus: None };
        let h = r.to_headers();
        assert!(h.contains("X-Spam-Score: 3.14"));
        assert!(h.contains("X-Spam-Status: No"));
        assert!(h.contains("X-Spam-Action: add header"));
        assert!(h.contains("X-Virus-Status: Clean"));
    }

    #[test]
    fn headers_infected() {
        let r = ScanResult { spam_score: None, spam_action: None, virus: Some("Test.Virus".into()) };
        let h = r.to_headers();
        assert!(h.contains("X-Virus-Status: Infected: Test.Virus"));
    }

    #[test]
    fn spam_high_score_flagged_yes() {
        let r = ScanResult { spam_score: Some(7.5), spam_action: Some("add header".into()), virus: None };
        assert!(r.to_headers().contains("X-Spam-Status: Yes"));
    }
}
