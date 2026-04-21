//! GET /auth/me → validated AuthContext JSON.

use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use expresso_auth_client::Authenticated;

#[derive(Debug, Serialize)]
pub struct MeResponse {
    pub user_id:      Uuid,
    pub tenant_id:    Uuid,
    pub email:        String,
    pub display_name: String,
    pub roles:        Vec<String>,
    /// Unix epoch seconds.
    pub expires_at:   i64,
}

pub async fn me(Authenticated(ctx): Authenticated) -> Json<MeResponse> {
    Json(MeResponse {
        user_id:      ctx.user_id,
        tenant_id:    ctx.tenant_id,
        email:        ctx.email,
        display_name: ctx.display_name,
        roles:        ctx.roles,
        expires_at:   ctx.expires_at,
    })
}
