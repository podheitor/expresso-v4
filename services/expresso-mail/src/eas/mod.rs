//! Exchange ActiveSync (EAS) server — endpoint, auth, command dispatch.
//!
//! One HTTP endpoint, `/Microsoft-Server-ActiveSync`:
//!  - `OPTIONS` advertises the supported protocol versions + commands.
//!  - `POST?Cmd=<X>&User=&DeviceId=&DeviceType=` carries a WBXML command body.
//!
//! This sprint wires the endpoint, HTTP Basic auth (shared `users` crypt() +
//! lockout), version negotiation, and the Provision command. Other commands
//! (FolderSync, Sync, Ping) land in later sprints; until then they return HTTP
//! 501 so a client fails cleanly rather than mis-parsing an empty body.
//!
//! Mounted only when `mail_server.activesync_enabled` is set — default off.

mod auth;
mod foldersync;
mod provision;

use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, options},
    Router,
};
use serde::Deserialize;
use tracing::{info, warn};

use crate::state::AppState;

/// Protocol versions we advertise. Targeting 14.1 only keeps the command
/// handling single-branch (no multi-version quirks).
const PROTOCOL_VERSIONS: &str = "14.1";
/// Commands advertised in the OPTIONS response. Only Provision is functional
/// this sprint; the rest are advertised so clients proceed past discovery.
const PROTOCOL_COMMANDS: &str =
    "Provision,FolderSync,Sync,Ping,GetItemEstimate,ItemOperations,SendMail";
const WBXML_CONTENT_TYPE: &str = "application/vnd.ms-sync.wbxml";

/// EAS query string: `?Cmd=Sync&User=u&DeviceId=d&DeviceType=t`.
#[derive(Debug, Deserialize)]
struct EasQuery {
    #[serde(rename = "Cmd")]
    cmd: Option<String>,
    #[serde(rename = "DeviceId")]
    device_id: Option<String>,
}

/// Routes for the ActiveSync endpoint. Merged at the HTTP-server root (the path
/// is fixed by the protocol, not under `/api/v1`).
pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/Microsoft-Server-ActiveSync",
        options(handle_options).fallback(any(handle_command)),
    )
}

/// `OPTIONS /Microsoft-Server-ActiveSync` — advertise versions + commands.
/// No auth required (discovery), no body.
async fn handle_options() -> Response {
    (
        StatusCode::OK,
        [
            ("MS-Server-ActiveSync", "14.1"),
            ("MS-ASProtocolVersions", PROTOCOL_VERSIONS),
            ("MS-ASProtocolCommands", PROTOCOL_COMMANDS),
            (header::ALLOW.as_str(), "OPTIONS,POST"),
        ],
        "",
    )
        .into_response()
}

/// `POST /Microsoft-Server-ActiveSync?Cmd=…` — authenticate, then dispatch.
async fn handle_command(
    State(state): State<AppState>,
    Query(q): Query<EasQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let authz = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let Some(principal) = auth::authenticate(&state, authz).await else {
        return unauthorized();
    };

    let cmd = q.cmd.as_deref().unwrap_or("");
    info!(
        cmd = %cmd,
        user_id = %principal.user_id,
        tenant_id = %principal.tenant_id,
        device = q.device_id.as_deref().unwrap_or("-"),
        "EAS command"
    );

    match cmd {
        "Provision" => wbxml_ok(provision::provision_response()),
        "FolderSync" => {
            let key = foldersync::parse_sync_key(&body);
            let resp = foldersync::foldersync_response(
                &state,
                principal.user_id,
                principal.tenant_id,
                &key,
            )
            .await;
            wbxml_ok(resp)
        }
        // Implemented in later sprints; return 501 with a clear status so a
        // client doesn't treat an empty 200 as a malformed response.
        "Sync" | "Ping" | "GetItemEstimate" | "ItemOperations" | "SendMail" => {
            warn!(cmd = %cmd, "EAS command not yet implemented");
            (
                StatusCode::NOT_IMPLEMENTED,
                format!("{cmd} not implemented"),
            )
                .into_response()
        }
        _ => (StatusCode::BAD_REQUEST, "unknown or missing Cmd").into_response(),
    }
}

/// 401 with the `WWW-Authenticate: Basic` challenge EAS clients expect.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Basic realm=\"ActiveSync\"")],
        "",
    )
        .into_response()
}

/// 200 with a WBXML body and the EAS content type.
fn wbxml_ok(body: Vec<u8>) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, WBXML_CONTENT_TYPE)],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_advertises_14_1() {
        assert_eq!(PROTOCOL_VERSIONS, "14.1");
        assert!(PROTOCOL_COMMANDS.contains("Provision"));
        assert!(PROTOCOL_COMMANDS.contains("FolderSync"));
    }

    #[test]
    fn wbxml_content_type_is_ms_sync() {
        assert_eq!(WBXML_CONTENT_TYPE, "application/vnd.ms-sync.wbxml");
    }

    #[test]
    fn eas_query_deser_capitalized_keys() {
        // EAS uses capitalized query keys (Cmd/DeviceId); confirm the serde
        // rename attributes map them. axum's Query uses serde_urlencoded, which
        // is in the dependency tree via axum.
        let q: EasQuery =
            serde_urlencoded::from_str("Cmd=Sync&DeviceId=abc&DeviceType=iPhone").unwrap();
        assert_eq!(q.cmd.as_deref(), Some("Sync"));
        assert_eq!(q.device_id.as_deref(), Some("abc"));
    }
}
