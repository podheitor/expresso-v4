//! GET /auth/me → validated AuthContext JSON.

use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use expresso_auth_client::Authenticated;

/// MFA snapshot derived from OIDC `amr` / `acr` claims (RFC 8176 + OIDC §2).
/// `totp`/`webauthn` reflect that the user performed step-up with that method
/// during this token's issuance — ≠ "enrolled in KC".
#[derive(Debug, Serialize)]
pub struct MfaInfo {
    pub totp:     bool,
    pub webauthn: bool,
    pub amr:      Vec<String>,
    pub acr:      Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user_id:      Uuid,
    pub tenant_id:    Uuid,
    pub email:        String,
    pub display_name: String,
    pub roles:        Vec<String>,
    /// Unix epoch seconds.
    pub expires_at:   i64,
    pub mfa:          MfaInfo,
}

pub async fn me(Authenticated(ctx): Authenticated) -> Json<MeResponse> {
    // AMR tokens per RFC 8176: "otp"→TOTP, "hwk"/"swk"→WebAuthn/security key.
    let amr_lower: Vec<String> = ctx.amr.iter().map(|s| s.to_ascii_lowercase()).collect();
    let totp = amr_lower.iter().any(|a| a == "otp" || a == "totp");
    let webauthn = amr_lower.iter().any(|a| a == "hwk" || a == "swk" || a == "webauthn" || a == "u2f");

    Json(MeResponse {
        user_id:      ctx.user_id,
        tenant_id:    ctx.tenant_id,
        email:        ctx.email,
        display_name: ctx.display_name,
        roles:        ctx.roles,
        expires_at:   ctx.expires_at,
        mfa:          MfaInfo { totp, webauthn, amr: ctx.amr, acr: ctx.acr },
    })
}
