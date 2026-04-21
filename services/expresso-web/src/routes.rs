//! HTTP routes — SSR pages + health.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use crate::{
    AppState,
    error::WebResult,
    templates::{
        Folder, LoginTpl, MailListTpl, Me, MeTpl, MessageDetail,
        MessageListItem, SecurityTpl,
    },
    upstream::get_json,
};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/",              get(index))
        .route("/healthz",       get(healthz))
        .route("/login",         get(login_page))
        .route("/me",            get(me_page))
        .route("/me/security",   get(security_page))
        .route("/mail",          get(mail_page))
        .route("/mail/:id",      get(mail_detail_page))
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK,
     [(header::CONTENT_TYPE, "application/json")],
     r#"{"service":"expresso-web","status":"ok"}"#)
}

async fn index() -> Redirect {
    Redirect::to("/mail")
}

fn login_redirect(uri: &Uri) -> Redirect {
    let target = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let enc = utf8_percent_encode(target, NON_ALPHANUMERIC).to_string();
    let url = format!("/login?redirect={enc}");
    Redirect::to(&url)
}

/// Fetch Me from auth backend. None → redirect to /login (unless `allow_anon`).
async fn require_me(state: &AppState, headers: &HeaderMap) -> WebResult<Option<Me>> {
    get_json::<Me>(state, &state.backends.auth, "/auth/me", headers).await
}

// ─── /login ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginQuery { redirect: Option<String>, error: Option<String> }

async fn login_page(
    State(st): State<AppState>,
    Query(q):  Query<LoginQuery>,
) -> WebResult<Response> {
    let redirect = q.redirect.unwrap_or_else(|| "/".into());
    let enc = utf8_percent_encode(&redirect, NON_ALPHANUMERIC).to_string();
    let login_url = format!("{}?redirect_uri={}", st.public.auth_login_path, enc);
    Ok(askama_axum::IntoResponse::into_response(LoginTpl { login_url, error: q.error }))
}

// ─── /me ─────────────────────────────────────────────────────────────────────

async fn me_page(
    State(st):    State<AppState>,
    headers:      HeaderMap,
    uri:          Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(MeTpl {
        me,
        logout_url: st.public.auth_logout_path.clone(),
    }))
}

// ─── /me/security ────────────────────────────────────────────────────────────

async fn security_page(
    State(st): State<AppState>,
    headers:   HeaderMap,
    uri:       Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(SecurityTpl {
        me,
        kc_account: st.public.kc_account.clone(),
    }))
}

// ─── /mail ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MailQuery { folder: Option<String>, page: Option<u32> }

async fn mail_page(
    State(st):   State<AppState>,
    headers:     HeaderMap,
    uri:         Uri,
    Query(q):    Query<MailQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let selected = q.folder.unwrap_or_else(|| "INBOX".into());
    let page     = q.page.unwrap_or(0);

    let folders = get_json::<Vec<Folder>>(
        &st, &st.backends.mail, "/v1/mail/folders", &headers,
    ).await?.unwrap_or_default();

    let enc_folder = utf8_percent_encode(&selected, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st, &st.backends.mail,
        &format!("/v1/mail/messages?folder={enc_folder}&page={page}"),
        &headers,
    ).await?.unwrap_or_default();

    Ok(askama_axum::IntoResponse::into_response(MailListTpl {
        me, folders, selected, messages,
        detail: None, selected_id: None,
    }))
}

async fn mail_detail_page(
    State(st):  State<AppState>,
    headers:    HeaderMap,
    uri:        Uri,
    Path(id):   Path<String>,
    Query(q):   Query<MailQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let selected = q.folder.unwrap_or_else(|| "INBOX".into());
    let page     = q.page.unwrap_or(0);

    let folders = get_json::<Vec<Folder>>(
        &st, &st.backends.mail, "/v1/mail/folders", &headers,
    ).await?.unwrap_or_default();

    let enc_folder = utf8_percent_encode(&selected, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st, &st.backends.mail,
        &format!("/v1/mail/messages?folder={enc_folder}&page={page}"),
        &headers,
    ).await?.unwrap_or_default();

    let enc_id = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let detail = get_json::<MessageDetail>(
        &st, &st.backends.mail,
        &format!("/v1/mail/messages/{enc_id}"),
        &headers,
    ).await?;

    Ok(askama_axum::IntoResponse::into_response(MailListTpl {
        me, folders, selected, messages,
        detail, selected_id: Some(id),
    }))
}
