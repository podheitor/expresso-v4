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
        AddressBook, Calendar, Contact, DriveFile, DriveQuota, Folder, LoginTpl, DriveShareTpl, DriveVersionsTpl, MailComposeTpl, MailListTpl, Me, MeTpl, HomeTpl, ShareRow, VersionRow,
        MessageDetail, MessageListItem, SecurityTpl, DriveTpl, DriveTrashTpl, DriveEditTpl, CalendarTpl, ContactsTpl,
        Event, MonthCell, CalendarMonthTpl, CalendarWeekTpl, CalendarDayTpl, DayColumn, EventFormTpl, ContactFormTpl,
        AclRow, CalendarShareTpl, AddrbookShareTpl,
    },
    upstream::{get_json, post_body, put_body, delete_at},
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
        .route("/calendar",                           get(calendar_page))
        .route("/calendar/:cal_id",                   get(calendar_month_page))
        .route("/calendar/:cal_id/week",              get(calendar_week_page))
        .route("/calendar/:cal_id/day",               get(calendar_day_page))
        .route("/calendar/:cal_id/events/new",        get(event_new_form).post(event_new_action))
        .route("/calendar/:cal_id/events/:id/edit",   get(event_edit_form).post(event_edit_action))
        .route("/calendar/:cal_id/events/:id/delete", post(event_delete_action))
        .route("/calendar/:cal_id/share", get(calendar_share_page).post(calendar_share_create))
        .route("/calendar/:cal_id/share/:grantee_id/revoke", post(calendar_share_revoke))
        .route("/contacts",                                 get(contacts_page))
        .route("/contacts/:book_id/new",                    get(contact_new_form).post(contact_new_action))
        .route("/contacts/:book_id/:id/edit",               get(contact_edit_form).post(contact_edit_action))
        .route("/contacts/:book_id/:id/delete",             post(contact_delete_action))
        .route("/contacts/:book_id/share", get(addrbook_share_page).post(addrbook_share_create))
        .route("/contacts/:book_id/share/:grantee_id/revoke", post(addrbook_share_revoke))
        .merge(expresso_observability::metrics_router())
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK,
     [(header::CONTENT_TYPE, "application/json")],
     r#"{"service":"expresso-web","status":"ok"}"#)
}

async fn index(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(HomeTpl { me }))
}

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
    // Build absolute redirect URL so auth-rp can issue a cross-host 303 back to web.
    let abs_redirect = if redirect.starts_with("http://") || redirect.starts_with("https://") {
        redirect
    } else if !st.public.web_base_url.is_empty() {
        format!("{}{}", st.public.web_base_url.trim_end_matches('/'), redirect)
    } else {
        redirect
    };
    let enc = utf8_percent_encode(&abs_redirect, NON_ALPHANUMERIC).to_string();
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

// ─── /calendar/:cal_id — month grid ─────────────────────────────────────────

use time::{Date, Month, OffsetDateTime, macros::format_description};

#[derive(Deserialize)]
struct MonthQuery { month: Option<String> }

/// Parse "YYYY-MM" → (year, month). Fallback: today.
fn parse_ym(s: Option<&str>) -> (i32, u8) {
    let today = OffsetDateTime::now_utc().date();
    let fallback = (today.year(), today.month() as u8);
    let Some(raw) = s else { return fallback };
    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 2 { return fallback; }
    let (Ok(y), Ok(m)) = (parts[0].parse::<i32>(), parts[1].parse::<u8>()) else { return fallback; };
    if !(1..=12).contains(&m) { return fallback; }
    (y, m)
}

fn month_label_pt(m: u8) -> &'static str {
    ["jan","fev","mar","abr","mai","jun","jul","ago","set","out","nov","dez"][(m as usize).saturating_sub(1).min(11)]
}

fn u8_to_month(m: u8) -> Month {
    match m { 1=>Month::January, 2=>Month::February, 3=>Month::March, 4=>Month::April,
              5=>Month::May, 6=>Month::June, 7=>Month::July, 8=>Month::August,
              9=>Month::September, 10=>Month::October, 11=>Month::November, _=>Month::December }
}

/// Add one month, wrapping year boundary.
fn next_ym(y: i32, m: u8) -> (i32, u8) { if m == 12 { (y+1, 1) } else { (y, m+1) } }
fn prev_ym(y: i32, m: u8) -> (i32, u8) { if m == 1 { (y-1, 12) } else { (y, m-1) } }

/// Build 6×7 cell grid — Monday-first.
fn build_weeks(year: i32, month: u8, events_by_day: &std::collections::HashMap<String, Vec<Event>>) -> Vec<Vec<MonthCell>> {
    let today = OffsetDateTime::now_utc().date();
    let first = Date::from_calendar_date(year, u8_to_month(month), 1).unwrap();
    let lead = first.weekday().number_from_monday() as i32 - 1;
    let start = first - time::Duration::days(lead as i64);
    (0..6).map(|w| (0..7).map(|d| {
        let offset = w * 7 + d;
        let day = start + time::Duration::days(offset as i64);
        let iso = day.format(format_description!("[year]-[month]-[day]")).unwrap();
        let in_month = day.month() as u8 == month && day.year() == year;
        let events = events_by_day.get(&iso).cloned().unwrap_or_default();
        MonthCell { iso: iso.clone(), day: day.day(), in_month, is_today: day == today, events }
    }).collect()).collect()
}

async fn calendar_month_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>, Query(q): Query<MonthQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let calendars = get_json::<Vec<Calendar>>(
        &st, &st.backends.calendar, "/api/v1/calendars", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let Some(selected) = calendars.iter().find(|c| c.id == cal_id).cloned() else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };

    let (y, m) = parse_ym(q.month.as_deref());

    // range = first day of month → first day of next month (UTC)
    let first = Date::from_calendar_date(y, u8_to_month(m), 1).unwrap();
    let (ny, nm) = next_ym(y, m);
    let next_first = Date::from_calendar_date(ny, u8_to_month(nm), 1).unwrap();
    let from = first.format(format_description!("[year]-[month]-[day]T00:00:00Z")).unwrap();
    let to   = next_first.format(format_description!("[year]-[month]-[day]T00:00:00Z")).unwrap();

    let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let events = get_json::<Vec<Event>>(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events?from={from}&to={to}"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    let mut by_day: std::collections::HashMap<String, Vec<Event>> = std::collections::HashMap::new();
    for ev in events {
        let key = ev.date_key();
        if !key.is_empty() { by_day.entry(key).or_default().push(ev); }
    }
    for v in by_day.values_mut() { v.sort_by(|a,b| a.dtstart.cmp(&b.dtstart)); }

    let weeks = build_weeks(y, m, &by_day);

    let (py, pm) = prev_ym(y, m);
    let (ny2, nm2) = (ny, nm);
    let prev_link  = format!("/calendar/{cal_id}?month={py:04}-{pm:02}");
    let next_link  = format!("/calendar/{cal_id}?month={ny2:04}-{nm2:02}");
    let today_link = format!("/calendar/{cal_id}");
    let month_label = format!("{} {:04}", month_label_pt(m), y);

    Ok(askama_axum::IntoResponse::into_response(CalendarMonthTpl {
        me, calendars, selected,
        year: y, month: m, month_label,
        prev_link, next_link, today_link,
        weekday_labels: vec!["Seg","Ter","Qua","Qui","Sex","Sáb","Dom"],
        weeks,
    }))
}

// ─── week / day views ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WeekQuery { start: Option<String> }

#[derive(Deserialize)]
struct DayQuery   { date:  Option<String> }

fn parse_iso_date(s: &str) -> Option<Date> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 { return None; }
    let y: i32 = s[0..4].parse().ok()?;
    let m: u8  = s[5..7].parse().ok()?;
    let d: u8  = s[8..10].parse().ok()?;
    Date::from_calendar_date(y, u8_to_month(m), d).ok()
}

fn weekday_pt(d: Date) -> &'static str {
    use time::Weekday::*;
    match d.weekday() {
        Monday=>"Seg", Tuesday=>"Ter", Wednesday=>"Qua", Thursday=>"Qui",
        Friday=>"Sex", Saturday=>"Sáb", Sunday=>"Dom",
    }
}

fn month_label_short(d: Date) -> String {
    format!("{:02}/{:02}", d.day(), d.month() as u8)
}

/// Fetch events from backend within [from, to).
async fn fetch_events(
    st: &AppState, headers: &HeaderMap, t: &str, u: &str,
    cal_id: &str, from: &str, to: &str,
) -> WebResult<Vec<Event>> {
    let enc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC).to_string();
    Ok(get_json::<Vec<Event>>(
        st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events?from={from}&to={to}"),
        headers, Some((t, u)),
    ).await?.unwrap_or_default())
}

async fn calendar_week_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>, Query(q): Query<WeekQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let calendars = get_json::<Vec<Calendar>>(
        &st, &st.backends.calendar, "/api/v1/calendars", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let Some(selected) = calendars.iter().find(|c| c.id == cal_id).cloned() else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };

    let today = OffsetDateTime::now_utc().date();
    let base = q.start.as_deref().and_then(parse_iso_date).unwrap_or(today);
    // Monday-first: back up (weekday-1) days.
    let lead = base.weekday().number_from_monday() as i64 - 1;
    let mon = base - time::Duration::days(lead);
    let sun = mon + time::Duration::days(6);

    let from = mon.format(format_description!("[year]-[month]-[day]T00:00:00Z")).unwrap();
    let to_d = mon + time::Duration::days(7);
    let to   = to_d.format(format_description!("[year]-[month]-[day]T00:00:00Z")).unwrap();

    let mut events = fetch_events(&st, &headers, &t, &u, &cal_id, &from, &to).await?;
    events.sort_by(|a,b| a.dtstart.cmp(&b.dtstart));

    let mut by_day: std::collections::HashMap<String, Vec<Event>> = std::collections::HashMap::new();
    for ev in events {
        let key = ev.date_key();
        if !key.is_empty() { by_day.entry(key).or_default().push(ev); }
    }

    let days: Vec<DayColumn> = (0..7).map(|i| {
        let d = mon + time::Duration::days(i);
        let iso = d.format(format_description!("[year]-[month]-[day]")).unwrap();
        let label = format!("{} {}", weekday_pt(d), month_label_short(d));
        DayColumn {
            events: by_day.remove(&iso).unwrap_or_default(),
            is_today: d == today,
            date_iso: iso,
            label,
        }
    }).collect();

    let prev = mon - time::Duration::days(7);
    let next = mon + time::Duration::days(7);
    let prev_link = format!("/calendar/{cal_id}/week?start={}", prev.format(format_description!("[year]-[month]-[day]")).unwrap());
    let next_link = format!("/calendar/{cal_id}/week?start={}", next.format(format_description!("[year]-[month]-[day]")).unwrap());
    let today_link = format!("/calendar/{cal_id}/week");
    let month_link = format!("/calendar/{cal_id}?month={}-{:02}", mon.year(), mon.month() as u8);
    let day_link   = format!("/calendar/{cal_id}/day?date={}", today.format(format_description!("[year]-[month]-[day]")).unwrap());
    let week_label = format!("{} – {}",
        mon.format(format_description!("[day]/[month]")).unwrap(),
        sun.format(format_description!("[day]/[month]/[year]")).unwrap());

    Ok(askama_axum::IntoResponse::into_response(CalendarWeekTpl {
        me, calendars, selected,
        week_label, prev_link, next_link, today_link, month_link, day_link,
        days,
    }))
}

async fn calendar_day_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>, Query(q): Query<DayQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let calendars = get_json::<Vec<Calendar>>(
        &st, &st.backends.calendar, "/api/v1/calendars", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let Some(selected) = calendars.iter().find(|c| c.id == cal_id).cloned() else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };

    let today = OffsetDateTime::now_utc().date();
    let d = q.date.as_deref().and_then(parse_iso_date).unwrap_or(today);
    let d_next = d + time::Duration::days(1);

    let from = d.format(format_description!("[year]-[month]-[day]T00:00:00Z")).unwrap();
    let to   = d_next.format(format_description!("[year]-[month]-[day]T00:00:00Z")).unwrap();
    let mut events = fetch_events(&st, &headers, &t, &u, &cal_id, &from, &to).await?;
    events.sort_by(|a,b| a.dtstart.cmp(&b.dtstart));

    let date_iso = d.format(format_description!("[year]-[month]-[day]")).unwrap();
    let prev = d - time::Duration::days(1);
    let next = d + time::Duration::days(1);
    let iso_fmt = format_description!("[year]-[month]-[day]");
    let prev_link  = format!("/calendar/{cal_id}/day?date={}", prev.format(iso_fmt).unwrap());
    let next_link  = format!("/calendar/{cal_id}/day?date={}", next.format(iso_fmt).unwrap());
    let today_link = format!("/calendar/{cal_id}/day");
    let week_link  = format!("/calendar/{cal_id}/week?start={}", d.format(iso_fmt).unwrap());
    let month_link = format!("/calendar/{cal_id}?month={}-{:02}", d.year(), d.month() as u8);
    let date_label = format!("{}, {:02}/{:02}/{:04}",
        weekday_pt(d), d.day(), d.month() as u8, d.year());

    Ok(askama_axum::IntoResponse::into_response(CalendarDayTpl {
        me, calendars, selected,
        date_label, date_iso,
        prev_link, next_link, today_link, week_link, month_link,
        events,
    }))
}

// ─── event create/edit/delete ───────────────────────────────────────────────

#[derive(Deserialize)]
struct EventForm {
    summary:     String,
    #[serde(default)] location:    String,
    #[serde(default)] description: String,
    dtstart:     String, // "YYYY-MM-DDTHH:MM"
    dtend:       String,
    #[serde(default)] attendees:   String, // newline / comma / semicolon separated
}

#[derive(Deserialize, Default)]
struct AttendeeRow {
    email: String,
    #[serde(default)]
    partstat: Option<String>,
}

#[derive(Deserialize)]
struct NewQuery { date: Option<String> }

async fn event_new_form(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>, Query(q): Query<NewQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let calendars = get_json::<Vec<Calendar>>(
        &st, &st.backends.calendar, "/api/v1/calendars", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let Some(calendar) = calendars.into_iter().find(|c| c.id == cal_id) else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };
    let date = q.date.unwrap_or_else(|| {
        OffsetDateTime::now_utc().date()
            .format(format_description!("[year]-[month]-[day]")).unwrap()
    });
    Ok(EventFormTpl {
        me, calendar, event_id: None,
        summary: String::new(), location: String::new(), description: String::new(),
        dtstart: format!("{date}T09:00"),
        dtend:   format!("{date}T10:00"),
        attendees: String::new(),
        attendee_pills: Vec::new(),
        error: None,
    }.into_response())
}

/// Convert "YYYY-MM-DDTHH:MM" → iCal "YYYYMMDDTHHMMSSZ" (assume UTC input for MVP).
fn local_to_ical_utc(s: &str) -> Option<String> {
    // accept "YYYY-MM-DDTHH:MM" or "YYYY-MM-DDTHH:MM:SS"
    let (date, rest) = s.split_once('T')?;
    let (h, m) = rest.get(0..2).zip(rest.get(3..5))?;
    let date_compact: String = date.chars().filter(|c| *c != '-').collect();
    Some(format!("{date_compact}T{h}{m}00Z"))
}

fn escape_ical(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace(',', "\\,").replace(';', "\\;")
}

fn parse_attendees(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('@'))
        .map(str::to_ascii_lowercase)
        .collect()
}

fn build_vcalendar(
    uid: &str,
    organizer_email: Option<&str>,
    attendees: &[String],
    method: Option<&str>,
    f: &EventForm,
) -> Option<String> {
    let dtstart = local_to_ical_utc(&f.dtstart)?;
    let dtend   = local_to_ical_utc(&f.dtend)?;
    let now     = OffsetDateTime::now_utc()
        .format(format_description!("[year][month][day]T[hour][minute][second]Z")).ok()?;
    let mut ical = String::new();
    ical.push_str("BEGIN:VCALENDAR\r\n");
    ical.push_str("VERSION:2.0\r\n");
    ical.push_str("PRODID:-//expresso//web//PT-BR\r\n");
    if let Some(m) = method {
        ical.push_str(&format!("METHOD:{m}\r\n"));
    }
    ical.push_str("BEGIN:VEVENT\r\n");
    ical.push_str(&format!("UID:{uid}\r\n"));
    ical.push_str(&format!("DTSTAMP:{now}\r\n"));
    ical.push_str(&format!("DTSTART:{dtstart}\r\n"));
    ical.push_str(&format!("DTEND:{dtend}\r\n"));
    if method == Some("CANCEL") {
        ical.push_str("STATUS:CANCELLED\r\n");
        ical.push_str("SEQUENCE:1\r\n");
    }
    ical.push_str(&format!("SUMMARY:{}\r\n", escape_ical(f.summary.trim())));
    if !f.location.trim().is_empty() {
        ical.push_str(&format!("LOCATION:{}\r\n", escape_ical(f.location.trim())));
    }
    if !f.description.trim().is_empty() {
        ical.push_str(&format!("DESCRIPTION:{}\r\n", escape_ical(f.description.trim())));
    }
    if let Some(email) = organizer_email {
        if !email.is_empty() {
            ical.push_str(&format!("ORGANIZER:mailto:{email}\r\n"));
        }
    }
    for a in attendees {
        ical.push_str(&format!(
            "ATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:{a}\r\n"
        ));
    }
    ical.push_str("END:VEVENT\r\n");
    ical.push_str("END:VCALENDAR\r\n");
    Some(ical)
}

async fn event_new_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>, Form(f): Form<EventForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let uid = format!("{}@expresso-web", uuid_v4_hex());
    let attendees = parse_attendees(&f.attendees);
    let Some(ical) = build_vcalendar(&uid, Some(&me.email), &attendees, None, &f) else {
        return Ok((StatusCode::BAD_REQUEST, "Datas inválidas").into_response());
    };
    let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let status = post_body(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events"),
        &headers, Some((&t, &u)),
        Bytes::from(ical), "text/calendar; charset=utf-8",
    ).await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    if !attendees.is_empty() {
        let Some(itip) = build_vcalendar(&uid, Some(&me.email), &attendees, Some("REQUEST"), &f) else {
            return Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response());
        };
        let _ = post_body(
            &st, &st.backends.calendar,
            "/api/v1/scheduling/send",
            &headers, Some((&t, &u)),
            Bytes::from(itip), "text/calendar; charset=utf-8",
        ).await?;
    }
    Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response())
}

async fn event_edit_form(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((cal_id, id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let calendars = get_json::<Vec<Calendar>>(
        &st, &st.backends.calendar, "/api/v1/calendars", &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let Some(calendar) = calendars.into_iter().find(|c| c.id == cal_id) else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };
    let enc_c = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_e = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let event: Event = match get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(e) => e,
        None => return Ok((StatusCode::NOT_FOUND, "Evento não encontrado").into_response()),
    };
    fn iso_to_local(s: &str) -> String {
        // "2026-05-01T10:00:00+00:00" → "2026-05-01T10:00"
        if s.len() >= 16 { s[..16].to_string() } else { s.to_string() }
    }
    let atts: Vec<AttendeeRow> = get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}/attendees"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    let attendees_text = atts.iter().map(|a| a.email.as_str()).collect::<Vec<_>>().join("\n");
    let attendee_pills = atts.iter().map(|a| crate::templates::AttendeePill {
        email:    a.email.clone(),
        partstat: a.partstat.clone().unwrap_or_else(|| "NEEDS-ACTION".into()).to_ascii_uppercase(),
    }).collect();
    Ok(EventFormTpl {
        me, calendar, event_id: Some(id),
        summary:     event.summary.unwrap_or_default(),
        location:    event.location.unwrap_or_default(),
        description: event.description.unwrap_or_default(),
        dtstart:     event.dtstart.as_deref().map(iso_to_local).unwrap_or_default(),
        dtend:       event.dtend.as_deref().map(iso_to_local).unwrap_or_default(),
        attendees:   attendees_text,
        attendee_pills,
        error: None,
    }.into_response())
}

async fn event_edit_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((cal_id, id)): Path<(String, String)>, Form(f): Form<EventForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    // fetch existing UID → preserve for replace
    let enc_c = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_e = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let existing: Event = match get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(e) => e,
        None => return Ok((StatusCode::NOT_FOUND, "Evento não encontrado").into_response()),
    };
    let attendees = parse_attendees(&f.attendees);
    let organizer = existing.organizer_email.as_deref().or(Some(me.email.as_str()));
    let Some(ical) = build_vcalendar(&existing.uid, organizer, &attendees, None, &f) else {
        return Ok((StatusCode::BAD_REQUEST, "Datas inválidas").into_response());
    };
    let status = put_body(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers, Some((&t, &u)),
        Bytes::from(ical), "text/calendar; charset=utf-8",
    ).await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    if !attendees.is_empty() {
        let Some(itip) = build_vcalendar(&existing.uid, organizer, &attendees, Some("REQUEST"), &f) else {
            return Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response());
        };
        let _ = post_body(
            &st, &st.backends.calendar,
            "/api/v1/scheduling/send",
            &headers, Some((&t, &u)),
            Bytes::from(itip), "text/calendar; charset=utf-8",
        ).await?;
    }
    Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response())
}

async fn event_delete_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((cal_id, id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_c = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_e = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();

    // Fetch event + attendees BEFORE delete to build CANCEL iTIP.
    let event_pre: Option<Event> = get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers, Some((&t, &u)),
    ).await?;
    let atts_pre: Vec<AttendeeRow> = get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}/attendees"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();

    let _ = delete_at(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers, Some((&t, &u)),
    ).await?;

    // After delete: if organizer is current user (or unset) AND there are attendees,
    // dispatch METHOD:CANCEL to all attendees so their clients withdraw.
    if let Some(ev) = event_pre {
        let is_organizer = ev.organizer_email.as_deref()
            .map(|o| o.eq_ignore_ascii_case(&me.email))
            .unwrap_or(true);
        let attendee_emails: Vec<String> = atts_pre.into_iter()
            .map(|a| a.email).filter(|e| !e.is_empty()).collect();
        if is_organizer && !attendee_emails.is_empty() {
            let f = EventForm {
                summary:     ev.summary.clone().unwrap_or_else(|| "(cancelado)".into()),
                location:    ev.location.clone().unwrap_or_default(),
                description: ev.description.clone().unwrap_or_default(),
                // dtstart/dtend back to "YYYY-MM-DDTHH:MM" so build_vcalendar can re-encode
                dtstart:     ev.dtstart.as_deref().map(|s| s.get(0..16).unwrap_or("").to_string()).unwrap_or_default(),
                dtend:       ev.dtend.as_deref().map(|s| s.get(0..16).unwrap_or("").to_string()).unwrap_or_default(),
                attendees:   String::new(),
            };
            let organizer = ev.organizer_email.as_deref().or(Some(me.email.as_str()));
            if let Some(itip) = build_vcalendar(&ev.uid, organizer, &attendee_emails, Some("CANCEL"), &f) {
                let _ = post_body(
                    &st, &st.backends.calendar,
                    "/api/v1/scheduling/send",
                    &headers, Some((&t, &u)),
                    Bytes::from(itip), "text/calendar; charset=utf-8",
                ).await?;
            }
        }
    }

    Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response())
}

/// Unique-enough UID for iCal VEVENTs — unix nanos as 32-hex.
fn uuid_v4_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{:032x}", nanos)
}

// ─── contacts CRUD ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ContactForm {
    #[serde(default)] full_name:    String,
    #[serde(default)] given_name:   String,
    #[serde(default)] family_name:  String,
    #[serde(default)] email:        String,
    #[serde(default)] phone:        String,
    #[serde(default)] organization: String,
}

fn escape_vcard(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace(',', "\\,").replace(';', "\\;")
}

fn build_vcard(uid: &str, f: &ContactForm) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCARD\r\n");
    out.push_str("VERSION:4.0\r\n");
    out.push_str(&format!("UID:{uid}\r\n"));
    // N: family;given;;; ;   FN: full_name (fallback to join)
    let family = escape_vcard(f.family_name.trim());
    let given  = escape_vcard(f.given_name.trim());
    out.push_str(&format!("N:{family};{given};;;\r\n"));
    let fn_value = if f.full_name.trim().is_empty() {
        format!("{} {}", f.given_name.trim(), f.family_name.trim()).trim().to_string()
    } else {
        f.full_name.trim().to_string()
    };
    if !fn_value.is_empty() { out.push_str(&format!("FN:{}\r\n", escape_vcard(&fn_value))); }
    if !f.email.trim().is_empty()    { out.push_str(&format!("EMAIL;TYPE=INTERNET:{}\r\n", escape_vcard(f.email.trim()))); }
    if !f.phone.trim().is_empty()    { out.push_str(&format!("TEL;TYPE=VOICE:{}\r\n", escape_vcard(f.phone.trim()))); }
    if !f.organization.trim().is_empty() { out.push_str(&format!("ORG:{}\r\n", escape_vcard(f.organization.trim()))); }
    out.push_str("END:VCARD\r\n");
    out
}

async fn load_book(st: &AppState, headers: &HeaderMap, t: &str, u: &str, book_id: &str) -> WebResult<Option<AddressBook>> {
    let books = get_json::<Vec<AddressBook>>(
        st, &st.backends.contacts, "/api/v1/addressbooks", headers, Some((t, u)),
    ).await?.unwrap_or_default();
    Ok(books.into_iter().find(|b| b.id == book_id))
}

async fn contact_new_form(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(book_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let Some(book) = load_book(&st, &headers, &t, &u, &book_id).await? else {
        return Ok((StatusCode::NOT_FOUND, "Catálogo não encontrado").into_response());
    };
    Ok(ContactFormTpl {
        me, book, contact_id: None,
        full_name: String::new(), given_name: String::new(), family_name: String::new(),
        email: String::new(), phone: String::new(), organization: String::new(),
        error: None,
    }.into_response())
}

async fn contact_new_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(book_id): Path<String>, Form(f): Form<ContactForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let uid = format!("web-{}", uuid_v4_hex());
    let vcard = build_vcard(&uid, &f);
    let enc = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let status = post_body(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}/contacts"),
        &headers, Some((&t, &u)),
        Bytes::from(vcard), "text/vcard; charset=utf-8",
    ).await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    let enc2 = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    Ok(Redirect::to(&format!("/contacts?book_id={enc2}")).into_response())
}

async fn contact_edit_form(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((book_id, id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let Some(book) = load_book(&st, &headers, &t, &u, &book_id).await? else {
        return Ok((StatusCode::NOT_FOUND, "Catálogo não encontrado").into_response());
    };
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let Some(contact): Option<Contact> = get_json(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers, Some((&t, &u)),
    ).await? else {
        return Ok((StatusCode::NOT_FOUND, "Contato não encontrado").into_response());
    };
    Ok(ContactFormTpl {
        me, book, contact_id: Some(id),
        full_name:    contact.full_name.unwrap_or_default(),
        given_name:   contact.given_name.unwrap_or_default(),
        family_name:  contact.family_name.unwrap_or_default(),
        email:        contact.email.unwrap_or_default(),
        phone:        contact.phone.unwrap_or_default(),
        organization: contact.organization.unwrap_or_default(),
        error: None,
    }.into_response())
}

async fn contact_edit_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((book_id, id)): Path<(String, String)>, Form(f): Form<ContactForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let Some(existing): Option<Contact> = get_json(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers, Some((&t, &u)),
    ).await? else {
        return Ok((StatusCode::NOT_FOUND, "Contato não encontrado").into_response());
    };
    let uid = existing.uid.clone().unwrap_or_else(|| format!("web-{}", uuid_v4_hex()));
    let vcard = build_vcard(&uid, &f);
    let status = put_body(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers, Some((&t, &u)),
        Bytes::from(vcard), "text/vcard; charset=utf-8",
    ).await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    Ok(Redirect::to(&format!("/contacts?book_id={enc_b}")).into_response())
}

async fn contact_delete_action(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((book_id, id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to(&format!("/contacts?book_id={enc_b}")).into_response())
}


// ─── ACL sharing pages ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ShareForm {
    email:     String,
    privilege: String,
}

#[derive(serde::Serialize)]
struct ShareJsonPayload<'a> {
    grantee_id: String,
    privilege:  &'a str,
}

#[derive(Deserialize)]
struct UserLookup { id: String, #[serde(default)] email: Option<String> }

async fn resolve_user_id(
    st: &AppState,
    backend: &str,
    email: &str,
    headers: &HeaderMap,
    t: &str,
    u: &str,
) -> WebResult<Option<String>> {
    let enc = utf8_percent_encode(email, NON_ALPHANUMERIC).to_string();
    let out: Option<UserLookup> = get_json(
        st, backend,
        &format!("/api/v1/users?email={enc}"),
        headers, Some((t, u)),
    ).await?;
    Ok(out.map(|x| { let _ = x.email; x.id }))
}

// ── Calendar share ──

async fn calendar_share_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let cal: Calendar = match get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(c) => c,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let shares: Vec<AclRow> = get_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/acl"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    Ok(CalendarShareTpl { me, calendar: cal, shares, error: None }.into_response())
}

async fn calendar_share_create(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(cal_id): Path<String>, Form(f): Form<ShareForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let email = f.email.trim().to_ascii_lowercase();
    let enc_cal = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();

    let grantee_id = match resolve_user_id(&st, &st.backends.calendar, &email, &headers, &t, &u).await? {
        Some(id) => id,
        None => return Ok(Redirect::to(&format!("/calendar/{enc_cal}/share?error=user_not_found")).into_response()),
    };

    let payload = ShareJsonPayload { grantee_id, privilege: &f.privilege };
    let status = crate::upstream::post_json(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_cal}/acl"),
        &headers, Some((&t, &u)), &payload,
    ).await?;
    if !(200..300).contains(&status) {
        return Ok(Redirect::to(&format!("/calendar/{enc_cal}/share?error=share_{status}")).into_response());
    }
    Ok(Redirect::to(&format!("/calendar/{enc_cal}/share")).into_response())
}

async fn calendar_share_revoke(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((cal_id, grantee_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_cal = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_g   = utf8_percent_encode(&grantee_id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st, &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_cal}/acl/{enc_g}"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to(&format!("/calendar/{enc_cal}/share")).into_response())
}

// ── Addressbook share ──

async fn addrbook_share_page(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(book_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let book: AddressBook = match get_json(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}"),
        &headers, Some((&t, &u)),
    ).await? {
        Some(b) => b,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let shares: Vec<AclRow> = get_json(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}/acl"),
        &headers, Some((&t, &u)),
    ).await?.unwrap_or_default();
    Ok(AddrbookShareTpl { me, addressbook: book, shares, error: None }.into_response())
}

async fn addrbook_share_create(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path(book_id): Path<String>, Form(f): Form<ShareForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let email = f.email.trim().to_ascii_lowercase();
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();

    let grantee_id = match resolve_user_id(&st, &st.backends.contacts, &email, &headers, &t, &u).await? {
        Some(id) => id,
        None => return Ok(Redirect::to(&format!("/contacts/{enc_b}/share?error=user_not_found")).into_response()),
    };
    let payload = ShareJsonPayload { grantee_id, privilege: &f.privilege };
    let status = crate::upstream::post_json(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/acl"),
        &headers, Some((&t, &u)), &payload,
    ).await?;
    if !(200..300).contains(&status) {
        return Ok(Redirect::to(&format!("/contacts/{enc_b}/share?error=share_{status}")).into_response());
    }
    Ok(Redirect::to(&format!("/contacts/{enc_b}/share")).into_response())
}

async fn addrbook_share_revoke(
    State(st): State<AppState>, headers: HeaderMap, uri: Uri,
    Path((book_id, grantee_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_g = utf8_percent_encode(&grantee_id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st, &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/acl/{enc_g}"),
        &headers, Some((&t, &u)),
    ).await?;
    Ok(Redirect::to(&format!("/contacts/{enc_b}/share")).into_response())
}
