//! HTTP routes — SSR pages.

use axum::{
    body::Bytes,
    extract::{Path, Query, State, Form},
    http::{HeaderMap, StatusCode, Uri, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use crate::{
    AppState,
    error::WebResult,
    templates::{
        AddressBook, Calendar, Contact, DriveFile, DriveQuota, Folder, LoginTpl, DriveShareTpl, DriveVersionsTpl, MailComposeTpl, MailListTpl, Me, MeTpl, ShareRow, VersionRow,
        MessageDetail, MessageListItem, SecurityTpl, DriveTpl, DriveTrashTpl, DriveEditTpl, CalendarTpl, ContactsTpl,
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
        .route("/mail/compose",  get(mail_compose_page).post(mail_compose_action))
        .route("/drive",            get(drive_page))
        .route("/drive/trash",      get(drive_trash_page))
        .route("/drive/upload",     post(drive_upload_action))
        .route("/drive/:id/trash",  post(drive_trash_action))
        .route("/drive/:id/restore",post(drive_restore_action))
        .route("/drive/:id/purge",  post(drive_purge_action))
        .route("/drive/:id/share",  get(drive_share_page).post(drive_share_create))
        .route("/drive/:id/share/:sid/revoke", post(drive_share_revoke))
        .route("/drive/:id/versions", get(drive_versions_page))
        .route("/drive/:id/edit",     get(drive_edit_page))
        .route("/calendar",      get(calendar_page))
        .route("/contacts",      get(contacts_page))
        .merge(expresso_observability::metrics_router())
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK,
     [(header::CONTENT_TYPE, "application/json")],
     r#"{"service":"expresso-web","status":"ok"}"#)
}

async fn index() -> Redirect { Redirect::to("/mail") }

fn login_redirect(uri: &Uri) -> Redirect {
    let target = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    let enc = utf8_percent_encode(target, NON_ALPHANUMERIC).to_string();
    Redirect::to(&format!("/login?redirect={enc}"))
}

async fn require_me(state: &AppState, headers: &HeaderMap) -> WebResult<Option<Me>> {
    get_json::<Me>(state, &state.backends.auth, "/auth/me", headers, None).await
}

fn ctx_of(me: &Me) -> (String, String) {
    (me.tenant_id.clone(), me.user_id.clone())
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

// ─── /me + /me/security ──────────────────────────────────────────────────────

async fn me_page(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(MeTpl {
        me, logout_url: st.public.auth_logout_path.clone(),
    }))
}

async fn security_page(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(SecurityTpl {
        me, kc_account: st.public.kc_account.clone(),
    }))
}

// ─── /mail ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MailQuery { folder: Option<String>, page: Option<u32> }

async fn mail_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri, Query(q): Query<MailQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let selected = q.folder.unwrap_or_else(|| "INBOX".into());
    let page     = q.page.unwrap_or(0);

    let folders = get_json::<Vec<Folder>>(
        &st, &st.backends.mail, "/api/v1/mail/folders", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    let enc = utf8_percent_encode(&selected, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st, &st.backends.mail,
        &format!("/api/v1/mail/messages?folder={enc}&page={page}"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    Ok(askama_axum::IntoResponse::into_response(MailListTpl {
        me, folders, selected, messages, detail: None, selected_id: None,
    }))
}

async fn mail_detail_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(id): Path<String>, Query(q): Query<MailQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let selected = q.folder.unwrap_or_else(|| "INBOX".into());
    let page     = q.page.unwrap_or(0);

    let folders = get_json::<Vec<Folder>>(
        &st, &st.backends.mail, "/api/v1/mail/folders", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    let enc = utf8_percent_encode(&selected, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st, &st.backends.mail,
        &format!("/api/v1/mail/messages?folder={enc}&page={page}"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    let enc_id = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let detail = get_json::<MessageDetail>(
        &st, &st.backends.mail,
        &format!("/api/v1/mail/messages/{enc_id}"),
        &headers, Some((&t, &u)),
    ).await?;

    Ok(askama_axum::IntoResponse::into_response(MailListTpl {
        me, folders, selected, messages, detail, selected_id: Some(id),
    }))
}

// ─── /drive ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DriveQuery { parent_id: Option<String> }

async fn drive_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri, Query(q): Query<DriveQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let path = match &q.parent_id {
        Some(p) if !p.is_empty() => {
            let enc = utf8_percent_encode(p, NON_ALPHANUMERIC).to_string();
            format!("/api/v1/drive/files?parent_id={enc}")
        }
        _ => "/api/v1/drive/files".into(),
    };
    let files = get_json::<Vec<DriveFile>>(
        &st, &st.backends.drive, &path, &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let quota = get_json::<DriveQuota>(
        &st, &st.backends.drive, "/api/v1/drive/quota", &headers, Some((&t, &u)),
    ).await?;

    Ok(askama_axum::IntoResponse::into_response(DriveTpl {
        me, parent_id: q.parent_id, files, quota,
    }))
}

async fn drive_trash_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let files = get_json::<Vec<DriveFile>>(
        &st, &st.backends.drive, "/api/v1/drive/trash", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(DriveTrashTpl { me, files }))
}

#[derive(Deserialize)]
struct UploadQuery { parent_id: Option<String> }

async fn drive_upload_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Query(q): Query<UploadQuery>, body: Bytes,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let ct = headers.get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let _ = crate::upstream::post_body(
        &st, &st.backends.drive, "/api/v1/drive/files",
        &headers, Some((&t, &u)), body, &ct,
    ).await?;
    let back = match &q.parent_id {
        Some(p) if !p.is_empty() => format!("/drive?parent_id={}", utf8_percent_encode(p, NON_ALPHANUMERIC)),
        _ => "/drive".into(),
    };
    Ok(Redirect::to(&back).into_response())
}

async fn drive_trash_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri, Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::delete_at(
        &st, &st.backends.drive, &format!("/api/v1/drive/files/{id}"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to("/drive").into_response())
}

async fn drive_restore_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri, Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::post_empty(
        &st, &st.backends.drive, &format!("/api/v1/drive/files/{id}/restore"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to("/drive/trash").into_response())
}

async fn drive_purge_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri, Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::delete_at(
        &st, &st.backends.drive, &format!("/api/v1/drive/files/{id}?permanent=true"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to("/drive/trash").into_response())
}

// ─── /calendar ───────────────────────────────────────────────────────────────

async fn calendar_page(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let calendars = get_json::<Vec<Calendar>>(
        &st, &st.backends.calendar, "/api/v1/calendars", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(CalendarTpl { me, calendars }))
}

// ─── /contacts ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ContactsQuery { book_id: Option<String> }

async fn contacts_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri, Query(q): Query<ContactsQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let books = get_json::<Vec<AddressBook>>(
        &st, &st.backends.contacts, "/api/v1/addressbooks", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    let selected_book = q.book_id.clone()
        .or_else(|| books.first().map(|b| b.id.clone()));

    let contacts = if let Some(bid) = &selected_book {
        let enc = utf8_percent_encode(bid, NON_ALPHANUMERIC).to_string();
        get_json::<Vec<Contact>>(
            &st, &st.backends.contacts,
            &format!("/api/v1/addressbooks/{enc}/contacts"),
            &headers, Some((&t, &u)),
        ).await?.unwrap_or_default()
    } else { Vec::new() };

    Ok(askama_axum::IntoResponse::into_response(ContactsTpl {
        me, books, selected_book, contacts,
    }))
}


// ─── /mail/compose ───────────────────────────────────────────────────────────

async fn mail_compose_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(MailComposeTpl { me, error: None }.into_response())
}

#[derive(Deserialize)]
struct ComposeForm {
    from:      String,
    to:        String,
    #[serde(default)] cc: String,
    subject:   String,
    body_text: String,
}

#[derive(serde::Serialize)]
struct SendPayload {
    from:      String,
    to:        Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cc:        Vec<String>,
    subject:   String,
    body_text: String,
}

fn split_addrs(s: &str) -> Vec<String> {
    s.split(|c: char| c == ',' || c == ';')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

async fn mail_compose_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Form(f): Form<ComposeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let to = split_addrs(&f.to);
    if to.is_empty() {
        return Ok(MailComposeTpl { me, error: Some("Informe ao menos um destinatário.".into()) }
            .into_response());
    }
    let payload = SendPayload {
        from: f.from, to, cc: split_addrs(&f.cc),
        subject: f.subject, body_text: f.body_text,
    };
    let status = crate::upstream::post_json(
        &st, &st.backends.mail, "/api/v1/mail/send",
        &headers, Some((&t, &u)), &payload,
    ).await?;
    if (200..300).contains(&(status as u16)) {
        Ok(Redirect::to("/mail").into_response())
    } else {
        Ok(MailComposeTpl {
            me,
            error: Some(format!("Falha ao enviar (HTTP {status}).")),
        }.into_response())
    }
}


// ─── /drive/:id/share ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SharePageQuery {
    new_url:   Option<String>,
    new_token: Option<String>,
}

async fn drive_share_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(id): Path<String>, Query(q): Query<SharePageQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st, &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(f) => f,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let shares: Vec<ShareRow> = get_json(
        &st, &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/shares"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    Ok(DriveShareTpl { me, file, shares, new_url: q.new_url, new_token: q.new_token }
        .into_response())
}

#[derive(Deserialize)]
struct ShareCreateForm { ttl_hours: i64 }

#[derive(serde::Serialize)]
struct ShareCreatePayload { expires_in_seconds: i64 }

#[derive(serde::Deserialize)]
struct ShareCreateResp {
    id:    String,
    token: String,
    url:   String,
}

async fn drive_share_create(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(id): Path<String>, Form(f): Form<ShareCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let ttl_s = f.ttl_hours.clamp(1, 720) * 3600;
    let payload = ShareCreatePayload { expires_in_seconds: ttl_s };
    // Precisamos do corpo de resposta → usa http client direto (não post_json que só retorna status).
    let url = format!("{}/api/v1/drive/files/{}/shares",
        st.backends.drive.trim_end_matches('/'), id);
    let mut req = st.http.post(&url).json(&payload);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        return Ok(Redirect::to(&format!("/drive/{id}/share?error={status}")).into_response());
    }
    let body: ShareCreateResp = resp.json().await?;
    let _ = body.id;
    let enc_url   = utf8_percent_encode(&body.url,   NON_ALPHANUMERIC).to_string();
    let enc_token = utf8_percent_encode(&body.token, NON_ALPHANUMERIC).to_string();
    Ok(Redirect::to(&format!("/drive/{id}/share?new_url={enc_url}&new_token={enc_token}"))
        .into_response())
}

async fn drive_share_revoke(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((id, sid)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::delete_at(
        &st, &st.backends.drive,
        &format!("/api/v1/drive/shares/{sid}"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to(&format!("/drive/{id}/share")).into_response())
}


// ─── /drive/:id/versions ─────────────────────────────────────────────────────

async fn drive_versions_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st, &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(f) => f,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let versions: Vec<VersionRow> = get_json(
        &st, &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/versions"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    Ok(DriveVersionsTpl { me, file, versions }.into_response())
}

// ─── /drive/:id/edit — WOPI/Collabora iframe ─────────────────────────────────

async fn drive_edit_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };

    if !st.wopi.is_enabled() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            "WOPI desabilitado — configure WOPI__SECRET no servidor"
        ).into_response());
    }

    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st, &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(f) => f,
        None => return Ok(login_redirect(&uri).into_response()),
    };

    if !file.is_editable() {
        return Ok((StatusCode::BAD_REQUEST,
            "Arquivo não suportado pelo editor (mime não editável).").into_response());
    }

    let token = crate::wopi::sign_token(
        st.wopi.secret.as_bytes(),
        &file.id, &me.tenant_id, &me.user_id,
        st.wopi.token_ttl_secs,
    );
    let iframe_url = crate::wopi::build_iframe_url(
        &st.wopi.collabora_url, &st.wopi.drive_url, &file.id, &token,
    );

    Ok(DriveEditTpl { me, file, iframe_url }.into_response())
}
