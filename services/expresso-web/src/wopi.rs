//! WOPI token emission + iframe URL builder.
//!
//! Formato MUST match expresso-drive/src/api/wopi.rs (sign_token/verify_token):
//! `{file_id}.{tenant_id}.{user_id}.{exp}.{hmac_hex}`.

use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use sha2::Sha256;
use time::OffsetDateTime;

type HmacSha256 = Hmac<Sha256>;

pub fn sign_token(
    secret:   &[u8],
    file_id:  &str,
    tenant:   &str,
    user:     &str,
    ttl_secs: i64,
) -> String {
    let exp = OffsetDateTime::now_utc().unix_timestamp() + ttl_secs;
    let canonical = format!("{file_id}|{tenant}|{user}|{exp}");
    let mut mac = HmacSha256::new_from_slice(secret).expect("hmac key");
    mac.update(canonical.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());
    format!("{file_id}.{tenant}.{user}.{exp}.{sig}")
}

/// Gera URL completa do iframe Collabora com WOPISrc + access_token.
pub fn build_iframe_url(
    collabora_base: &str,
    drive_base:     &str,
    file_id:        &str,
    access_token:   &str,
) -> String {
    let wopi_src = format!("{}/wopi/files/{}", drive_base.trim_end_matches('/'), file_id);
    let enc_src  = utf8_percent_encode(&wopi_src, NON_ALPHANUMERIC).to_string();
    let enc_tok  = utf8_percent_encode(access_token, NON_ALPHANUMERIC).to_string();
    format!(
        "{}/browser/dist/cool.html?WOPISrc={}&access_token={}",
        collabora_base.trim_end_matches('/'), enc_src, enc_tok,
    )
}

/// Checa se o mime é editável pelo Collabora.
pub fn is_editable_mime(mime: Option<&str>) -> bool {
    let Some(m) = mime else { return false; };
    matches!(m,
        "application/vnd.oasis.opendocument.text"
      | "application/vnd.oasis.opendocument.spreadsheet"
      | "application/vnd.oasis.opendocument.presentation"
      | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
      | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
      | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
      | "application/msword"
      | "application/vnd.ms-excel"
      | "application/vnd.ms-powerpoint"
      | "application/rtf"
      | "text/plain"
      | "text/csv"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editable_mimes() {
        assert!(is_editable_mime(Some("application/vnd.oasis.opendocument.text")));
        assert!(is_editable_mime(Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")));
        assert!(!is_editable_mime(Some("image/png")));
        assert!(!is_editable_mime(None));
    }

    #[test]
    fn token_format_matches_drive_verifier() {
        let tok = sign_token(b"secret", "f", "t", "u", 60);
        let parts: Vec<_> = tok.split('.').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0], "f");
        assert_eq!(parts[1], "t");
        assert_eq!(parts[2], "u");
    }

    #[test]
    fn iframe_url_encoding() {
        let url = build_iframe_url("http://co", "http://drive", "abc", "tok.abc");
        assert!(url.starts_with("http://co/browser/dist/cool.html?WOPISrc="));
        assert!(url.contains("http%3A%2F%2Fdrive%2Fwopi%2Ffiles%2Fabc"));
        assert!(url.contains("access_token=tok%2Eabc"));
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn editable_mime_text_plain() {
        assert!(is_editable_mime(Some("text/plain")));
    }

    #[test]
    fn editable_mime_text_csv() {
        assert!(is_editable_mime(Some("text/csv")));
    }

    #[test]
    fn editable_mime_msword() {
        assert!(is_editable_mime(Some("application/msword")));
    }

    #[test]
    fn editable_mime_pdf_not_editable() {
        assert!(!is_editable_mime(Some("application/pdf")));
    }

    #[test]
    fn editable_mime_empty_string_not_editable() {
        assert!(!is_editable_mime(Some("")));
    }

    #[test]
    fn sign_token_5_parts() {
        let tok = sign_token(b"key", "file", "tenant", "user", 3600);
        assert_eq!(tok.split('.').count(), 5);
    }

    #[test]
    fn build_iframe_url_contains_wopi_src_param() {
        let url = build_iframe_url("https://collabora", "https://drive", "file1", "tok1");
        assert!(url.contains("WOPISrc="));
        assert!(url.contains("access_token="));
    }

    #[test]
    fn build_iframe_url_contains_file_id() {
        let url = build_iframe_url("https://collabora", "https://drive", "my-file-id", "tok");
        assert!(url.contains("my-file-id"));
    }

    #[test]
    fn build_iframe_url_contains_token() {
        let url = build_iframe_url("https://collabora", "https://drive", "f1", "secret-token");
        assert!(url.contains("secret-token"));
    }

    #[test]
    fn build_iframe_url_starts_with_collabora_base() {
        let url = build_iframe_url("https://collabora.example.com", "https://drive", "file1", "tok");
        assert!(url.starts_with("https://collabora.example.com"));
    }

    #[test]
    fn build_iframe_url_contains_specific_file_id() {
        let url = build_iframe_url("https://collabora.example.com", "https://drive", "file-xyz", "tok");
        assert!(url.contains("file-xyz"));
    }

    #[test]
    fn is_editable_mime_none_returns_false() {
        assert!(!is_editable_mime(None));
    }

    #[test]
    fn build_iframe_url_contains_access_token_param() {
        let url = build_iframe_url("https://collabora", "https://drive", "file1", "mytoken");
        assert!(url.contains("access_token=mytoken"));
    }
}
