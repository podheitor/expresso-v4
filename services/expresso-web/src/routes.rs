//! HTTP routes — SSR pages.

use axum::{
    body::Bytes,
    extract::{Form, Path, Query, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use crate::{
    error::WebResult,
    templates::{
        AclRow, AddrbookShareTpl, AddressBook, AdminAuditTpl, AdminConfig, AdminConfigTpl,
        AdminLoginEvent, AdminMonitoringTpl, AdminTenant, AdminTenantsTpl, AdminUser,
        AdminUserDetailTpl, AdminUsersTpl, AuditEvent, Calendar, CalendarDayTpl, CalendarMonthTpl,
        CalendarShareTpl, CalendarTpl, CalendarWeekTpl, ChatChannel, ChatMessage, ChatTpl, Contact,
        ContactFormTpl, ContactGroup, ContactGroupDetailTpl, ContactGroupsTpl, ContactsTpl,
        DayColumn, DelegationRaw, DelegationView, DelegationsTpl, DriveEditTpl, DriveFile,
        DriveFileTag, DrivePreviewTpl, DriveQuota, DriveShareTpl, DriveTagFilesTpl, DriveTagStat,
        DriveTagsTpl, DriveTpl, DriveTrashTpl, DriveVersionsTpl, Event, EventFormTpl, Folder,
        GalContact, HomeDriveFile, HomeEvent, HomeTpl, LoginTpl, MailAlias, MailComposeTpl,
        MailListTpl, MailSearchTpl, MailThreadTpl, Me, MeTpl, MeetParticipant, MeetRoom,
        MeetRoomTpl, MeetScheduleTpl, MeetTpl, MessageDetail, MessageListItem, MonthCell, Note,
        NotesTpl, SearchGroup, SearchHit, SearchTpl, SecurityTpl, SettingsTpl, ShareRow, TasksTpl,
        VersionRow, WorkingHour,
    },
    upstream::{
        delete_at, get_bytes, get_json, patch_json, post_body, post_empty, post_json, put_body,
        put_json,
    },
    AppState,
};

fn dedup_folders(mut folders: Vec<crate::templates::Folder>) -> Vec<crate::templates::Folder> {
    let mut seen = std::collections::HashSet::new();
    folders.retain(|f| seen.insert(f.name.to_uppercase()));
    folders
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/healthz", get(healthz))
        .route("/login", get(login_page))
        .route("/me", get(me_page))
        .route("/me/security", get(security_page))
        .route("/mail", get(mail_page))
        .route(
            "/mail/compose",
            get(mail_compose_page).post(mail_compose_action),
        )
        .route("/mail/rules", get(mail_rules_page).post(mail_rules_save))
        .route("/mail/thread/:tid", get(mail_thread_page))
        .route("/mail/:id", get(mail_detail_page))
        .route("/drive", get(drive_page))
        .route("/drive/trash", get(drive_trash_page))
        .route("/drive/upload", post(drive_upload_action))
        .route("/drive/:id/trash", post(drive_trash_action))
        .route("/drive/:id/restore", post(drive_restore_action))
        .route("/drive/:id/purge", post(drive_purge_action))
        .route(
            "/drive/:id/share",
            get(drive_share_page).post(drive_share_create),
        )
        .route("/drive/:id/share/:sid/revoke", post(drive_share_revoke))
        .route("/drive/:id/versions", get(drive_versions_page))
        .route(
            "/drive/:id/versions/:vno/restore",
            post(drive_version_restore),
        )
        .route("/drive/:id/tags/add", post(drive_tag_add_action))
        .route("/drive/:id/tags/remove", post(drive_tag_remove_action))
        .route("/drive/:id/preview", get(drive_preview_page))
        .route("/drive/:id/edit", get(drive_edit_page))
        .route("/calendar", get(calendar_page))
        .route("/calendar/:cal_id", get(calendar_month_page))
        .route("/calendar/:cal_id/week", get(calendar_week_page))
        .route("/calendar/:cal_id/day", get(calendar_day_page))
        .route(
            "/calendar/:cal_id/events/new",
            get(event_new_form).post(event_new_action),
        )
        .route(
            "/calendar/:cal_id/events/:id/edit",
            get(event_edit_form).post(event_edit_action),
        )
        .route(
            "/calendar/:cal_id/events/:id/delete",
            post(event_delete_action),
        )
        .route("/calendar/:cal_id/events/:id/rsvp", post(event_rsvp_action))
        .route(
            "/calendar/:cal_id/share",
            get(calendar_share_page).post(calendar_share_create),
        )
        .route(
            "/calendar/:cal_id/share/:grantee_id/revoke",
            post(calendar_share_revoke),
        )
        .route("/calendar/:cal_id/export.ics", get(calendar_export_ics))
        .route("/calendar/:cal_id/import", post(calendar_import_ics))
        .route(
            "/calendar/:cal_id/events/:id/reschedule",
            post(event_reschedule_action),
        )
        .route(
            "/calendar/:cal_id/events/:id/extend",
            post(event_extend_action),
        )
        .route("/contacts", get(contacts_page))
        .route(
            "/contacts/:book_id/new",
            get(contact_new_form).post(contact_new_action),
        )
        .route(
            "/contacts/:book_id/:id/edit",
            get(contact_edit_form).post(contact_edit_action),
        )
        .route("/contacts/:book_id/:id/delete", post(contact_delete_action))
        .route(
            "/contacts/:book_id/share",
            get(addrbook_share_page).post(addrbook_share_create),
        )
        .route(
            "/contacts/:book_id/share/:grantee_id/revoke",
            post(addrbook_share_revoke),
        )
        .route("/contacts/:book_id/export.vcf", get(contacts_export_vcf))
        .route("/contacts/:book_id/import", post(contacts_import_vcf))
        // contact groups (distribution lists) — server-backed
        .route(
            "/contacts/groups",
            get(contact_groups_page).post(contact_group_create_action),
        )
        .route("/contacts/groups/:id", get(contact_group_detail_page))
        .route(
            "/contacts/groups/:id/rename",
            post(contact_group_rename_action),
        )
        .route(
            "/contacts/groups/:id/delete",
            post(contact_group_delete_action),
        )
        .route(
            "/contacts/groups/:id/members/add",
            post(contact_group_add_member_action),
        )
        .route(
            "/contacts/groups/:id/members/:cid/remove",
            post(contact_group_remove_member_action),
        )
        // mail extras
        .route("/search", get(unified_search_page))
        .route("/api/search", get(unified_search_api))
        .route("/mail/search", get(mail_search_page))
        .route("/mail/:id/attachments/:idx", get(mail_attachment_proxy))
        .route("/mail/quick-reply", post(mail_quick_reply_action))
        .route("/mail/:id/flag", post(mail_flag_action))
        .route("/mail/:id/move", post(mail_move_action))
        .route("/mail/:id/delete", post(mail_delete_action))
        .route("/mail/:id/snooze", post(mail_snooze_action))
        // mail folder management
        .route("/mail/folders/create", post(mail_folder_create_action))
        .route("/mail/folders/rename", post(mail_folder_rename_action))
        .route("/mail/folders/delete", post(mail_folder_delete_action))
        // drive extras
        .route("/drive/search", get(drive_search_page))
        .route("/drive/new-folder", post(drive_mkdir_action))
        .route("/drive/:id/rename", post(drive_rename_action))
        .route("/drive/:id/move", post(drive_move_action))
        .route("/drive/bulk-move", post(drive_bulk_move_action))
        .route("/drive/bulk-copy", post(drive_bulk_copy_action))
        .route("/drive/tags", get(drive_tags_page))
        .route("/drive/tags/:tag", get(drive_tag_files_page))
        // contacts extras
        .route("/contacts/gal", get(contacts_gal_page))
        // chat / meet
        .route("/chat", get(chat_page))
        .route("/chat/channels", post(chat_create_channel))
        .route("/chat/channels/:cid", get(chat_channel_page))
        .route("/chat/channels/:cid/send", post(chat_send_message))
        .route("/chat/channels/:cid/poll", get(chat_poll_messages))
        .route("/chat/channels/:cid/mark-read", post(chat_mark_read))
        .route(
            "/chat/channels/:cid/messages/:mid/react",
            post(chat_react_message),
        )
        .route(
            "/chat/channels/:cid/pin",
            get(chat_get_pin).post(chat_set_pin).delete(chat_delete_pin),
        )
        .route("/meet", get(meet_page))
        .route("/meet/new", get(meet_new_page).post(meet_create_action))
        .route(
            "/meet/schedule",
            get(meet_schedule_page).post(meet_schedule_action),
        )
        .route("/meet/join", get(meet_join_page))
        .route("/meet/:id", get(meet_room_page))
        .route("/meet/:id/end", post(meet_end_action))
        .route("/meet/:id/recordings", get(meet_recordings_api))
        // tasks
        .route("/tasks", get(tasks_page))
        .route("/notes", get(notes_page).post(notes_create_action))
        .route("/notes/:id", post(notes_edit_action))
        .route("/notes/:id/delete", post(notes_delete_action))
        // settings
        .route("/settings", get(settings_page))
        .route("/settings/profile", post(settings_profile_save))
        .route("/settings/signature", post(settings_signature_save))
        .route("/settings/autoreply", post(settings_autoreply_save))
        .route("/settings/notifications", post(settings_notifications_save))
        .route("/settings/filters", post(settings_filters_save))
        .route("/settings/working-hours", post(settings_working_hours_save))
        .route(
            "/settings/delegations",
            get(delegations_page).post(delegation_grant_action),
        )
        .route(
            "/settings/delegations/:id/revoke",
            post(delegation_revoke_action),
        )
        .route("/settings/aliases", post(settings_alias_create))
        .route("/settings/aliases/:id/toggle", post(settings_alias_toggle))
        .route("/settings/aliases/:id/delete", post(settings_alias_delete))
        // GAL autocomplete JSON API
        .route("/api/gal/search", get(gal_search_api))
        // Mail attachment list (JSON for JS)
        .route("/api/mail/:id/attachments", get(mail_attachments_api))
        // Admin panel
        .route("/admin", get(admin_redirect))
        .route("/admin/users", get(admin_users_page))
        .route("/admin/users/invite", post(admin_users_invite))
        .route("/admin/users/:id", get(admin_user_detail_page))
        .route("/admin/users/:id/quota", post(admin_user_set_quota))
        .route(
            "/admin/users/:id/sessions/revoke",
            post(admin_user_revoke_sessions),
        )
        .route("/admin/users/:id/role", post(admin_users_set_role))
        .route("/admin/users/:id/suspend", post(admin_users_suspend))
        .route("/admin/users/:id/activate", post(admin_users_activate))
        .route(
            "/admin/users/:id/reset-password",
            post(admin_users_reset_password),
        )
        .route(
            "/admin/tenants",
            get(admin_tenants_page).post(admin_tenants_create),
        )
        .route("/admin/tenants/:id/toggle", post(admin_tenants_toggle))
        .route("/admin/monitoring", get(admin_monitoring_page))
        .route("/admin/audit", get(admin_audit_page))
        .route(
            "/admin/config",
            get(admin_config_page).post(admin_config_save),
        )
        .route("/admin/api/stats", get(admin_api_stats))
        .route("/admin/api/audit", get(admin_api_audit))
        .route(
            "/admin/api/domain-quotas",
            get(admin_api_domain_quotas).put(admin_api_domain_quotas_save),
        )
        .route("/admin/api/smtp-queue", get(admin_api_smtp_queue))
        .route(
            "/admin/api/smtp-queue/flush",
            post(admin_api_smtp_queue_flush),
        )
        .merge(expresso_observability::metrics_router())
        .with_state(state)
}

async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        r#"{"service":"expresso-web","status":"ok"}"#,
    )
}

async fn index(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    // Mail: fetch INBOX unread count
    let (mail_unread, inbox_id) = {
        let folders: Vec<Folder> = {
            let mut req = st.http.get(format!(
                "{}/api/v1/folders",
                st.backends.mail.trim_end_matches('/')
            ));
            req = crate::upstream::fwd_cookie(req, &headers);
            req = crate::upstream::inject_ctx(req, &t, &u);
            match req.send().await {
                Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        let inbox = folders
            .iter()
            .find(|f| f.special_use.as_deref() == Some("\\Inbox"))
            .or_else(|| {
                folders
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case("INBOX"))
            });
        (
            inbox.map(|f| f.unseen_count).unwrap_or(0),
            inbox.map(|f| f.id.clone()).unwrap_or_default(),
        )
    };

    // Calendar: next 5 events today + tomorrow
    let events: Vec<HomeEvent> = {
        let now = chrono_now_iso();
        let date_prefix = &now[..10];
        let calendars: Vec<Calendar> = {
            let mut req = st.http.get(format!(
                "{}/api/v1/calendars",
                st.backends.calendar.trim_end_matches('/')
            ));
            req = crate::upstream::fwd_cookie(req, &headers);
            req = crate::upstream::inject_ctx(req, &t, &u);
            match req.send().await {
                Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        let default_cal = calendars
            .iter()
            .find(|c| c.is_default)
            .or_else(|| calendars.first());
        if let Some(cal) = default_cal {
            let url = format!(
                "{}/api/v1/calendars/{}/events?start={}&limit=5",
                st.backends.calendar.trim_end_matches('/'),
                cal.id,
                date_prefix
            );
            let mut req = st.http.get(&url);
            req = crate::upstream::fwd_cookie(req, &headers);
            req = crate::upstream::inject_ctx(req, &t, &u);
            match req.send().await {
                Ok(r) if r.status().is_success() => {
                    let raw: Vec<Event> = r.json().await.unwrap_or_default();
                    raw.into_iter()
                        .map(|e| {
                            let starts = e
                                .dtstart
                                .as_ref()
                                .map(|s| {
                                    if s.len() >= 16 {
                                        s[11..16].to_string()
                                    } else {
                                        s.clone()
                                    }
                                })
                                .unwrap_or_default();
                            let is_meet = e
                                .location
                                .as_deref()
                                .map(|l| {
                                    l.contains("/meet/")
                                        || l.contains("jitsi")
                                        || l.contains("expresso.local")
                                })
                                .unwrap_or(false);
                            let meet_room_id = if is_meet {
                                e.location
                                    .as_deref()
                                    .and_then(|l| l.split("/meet/").nth(1))
                                    .map(|s| s.split('/').next().unwrap_or("").to_string())
                                    .filter(|s| !s.is_empty())
                            } else {
                                None
                            };
                            HomeEvent {
                                id: e.id,
                                calendar_id: e.calendar_id,
                                summary: e.summary.unwrap_or_else(|| "(sem título)".into()),
                                starts,
                                is_meet,
                                meet_room_id,
                            }
                        })
                        .take(5)
                        .collect()
                }
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        }
    };

    // Drive: recent 5 files
    let drive_files: Vec<HomeDriveFile> = {
        let url = format!(
            "{}/api/v1/files?limit=5&sort=updated_at&order=desc",
            st.backends.drive.trim_end_matches('/')
        );
        let mut req = st.http.get(&url);
        req = crate::upstream::fwd_cookie(req, &headers);
        req = crate::upstream::inject_ctx(req, &t, &u);
        match req.send().await {
            Ok(r) if r.status().is_success() => {
                let raw: Vec<DriveFile> = r.json().await.unwrap_or_default();
                raw.into_iter()
                    .map(|f| HomeDriveFile {
                        id: f.id,
                        name: f.name,
                        kind: f.kind,
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    };

    // Chat: total unread across channels
    let chat_unread: i64 = {
        let channels: Vec<ChatChannel> = {
            let url = format!("{}/api/v1/channels", st.backends.chat.trim_end_matches('/'));
            let mut req = st.http.get(&url);
            req = crate::upstream::fwd_cookie(req, &headers);
            req = crate::upstream::inject_ctx(req, &t, &u);
            match req.send().await {
                Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        channels.iter().map(|c| c.unread_count).sum()
    };

    Ok(askama_axum::IntoResponse::into_response(HomeTpl {
        me,
        mail_unread,
        inbox_id,
        events,
        drive_files,
        chat_unread,
    }))
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
struct LoginQuery {
    redirect: Option<String>,
    error: Option<String>,
}

async fn login_page(
    State(st): State<AppState>,
    Query(q): Query<LoginQuery>,
) -> WebResult<Response> {
    let redirect = q.redirect.unwrap_or_else(|| "/".into());
    // Build absolute redirect URL so auth-rp can issue a cross-host 303 back to web.
    let abs_redirect = if redirect.starts_with("http://") || redirect.starts_with("https://") {
        redirect
    } else if !st.public.web_base_url.is_empty() {
        format!(
            "{}{}",
            st.public.web_base_url.trim_end_matches('/'),
            redirect
        )
    } else {
        redirect
    };
    let enc = utf8_percent_encode(&abs_redirect, NON_ALPHANUMERIC).to_string();
    let login_url = format!("{}?redirect_uri={}", st.public.auth_login_path, enc);
    Ok(askama_axum::IntoResponse::into_response(LoginTpl {
        login_url,
        error: q.error,
    }))
}

// ─── /me + /me/security ──────────────────────────────────────────────────────

async fn me_page(State(st): State<AppState>, headers: HeaderMap, uri: Uri) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(MeTpl {
        me,
        logout_url: st.public.auth_logout_path.clone(),
    }))
}

async fn security_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(SecurityTpl {
        me,
        kc_account: st.public.kc_account.clone(),
    }))
}

// ─── /mail ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MailQuery {
    folder: Option<String>,
    page: Option<u32>,
    json: Option<u8>,
    /// When set (a delegated owner's user id), view that mailbox instead of the
    /// caller's own. Requires a delegation grant (the backend 403s otherwise).
    obo: Option<String>,
}

/// Append `&on_behalf_of=<id>` to a backend path when viewing a delegated
/// mailbox. Returns the path unchanged when `obo` is None/blank.
fn with_obo(path: String, obo: Option<&str>) -> String {
    match obo.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            let enc = utf8_percent_encode(id, NON_ALPHANUMERIC).to_string();
            let sep = if path.contains('?') { '&' } else { '?' };
            format!("{path}{sep}on_behalf_of={enc}")
        }
        None => path,
    }
}

async fn mail_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<MailQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let selected = q.folder.unwrap_or_else(|| "INBOX".into());
    let page = q.page.unwrap_or(0);
    let obo = q.obo.as_deref();

    let folders = dedup_folders(
        get_json::<Vec<Folder>>(
            &st,
            &st.backends.mail,
            &with_obo("/api/v1/mail/folders".to_string(), obo),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default(),
    );

    let enc = utf8_percent_encode(&selected, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st,
        &st.backends.mail,
        &with_obo(
            format!("/api/v1/mail/messages?folder={enc}&page={page}"),
            obo,
        ),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    // Resolve the delegated owner's email for the "viewing as" banner.
    let viewing_as = match obo.map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => Some(resolve_email_by_id(&st, id, &headers, &t, &u).await),
        None => None,
    };

    let has_next = messages.len() >= 50; // backend page size
    Ok(askama_axum::IntoResponse::into_response(MailListTpl {
        me,
        folders,
        selected,
        messages,
        detail: None,
        selected_id: None,
        page,
        has_next,
        viewing_as,
        obo: obo.map(str::to_string),
    }))
}

async fn mail_detail_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Query(q): Query<MailQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let selected = q.folder.unwrap_or_else(|| "INBOX".into());
    let page = q.page.unwrap_or(0);
    let obo = q.obo.as_deref();

    let folders = dedup_folders(
        get_json::<Vec<Folder>>(
            &st,
            &st.backends.mail,
            &with_obo("/api/v1/mail/folders".to_string(), obo),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default(),
    );

    let enc = utf8_percent_encode(&selected, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st,
        &st.backends.mail,
        &with_obo(
            format!("/api/v1/mail/messages?folder={enc}&page={page}"),
            obo,
        ),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let enc_id = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let detail = get_json::<MessageDetail>(
        &st,
        &st.backends.mail,
        &with_obo(format!("/api/v1/mail/messages/{enc_id}"), obo),
        &headers,
        Some((&t, &u)),
    )
    .await?;

    // Return JSON for thread inline body loading
    if q.json == Some(1) {
        let json = serde_json::to_string(&detail).unwrap_or_else(|_| "null".into());
        return Ok((
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            json,
        )
            .into_response());
    }

    let viewing_as = match obo.map(str::trim).filter(|s| !s.is_empty()) {
        Some(oid) => Some(resolve_email_by_id(&st, oid, &headers, &t, &u).await),
        None => None,
    };

    Ok(askama_axum::IntoResponse::into_response(MailListTpl {
        me,
        folders,
        selected,
        messages,
        detail,
        selected_id: Some(id),
        page: 0,
        has_next: false,
        viewing_as,
        obo: obo.map(str::to_string),
    }))
}

// ─── /api/mail/:id/attachments ───────────────────────────────────────────────

#[allow(unused_variables)] // uri unused here; shared handler signature
async fn mail_attachments_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok((StatusCode::UNAUTHORIZED, "[]").into_response());
    };
    let (t, u) = ctx_of(&me);
    let list = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{id}/attachments"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    // Inject size_human for JS convenience
    let enriched: Vec<serde_json::Value> = list
        .into_iter()
        .map(|mut a| {
            let size = a.get("size").and_then(|s| s.as_i64()).unwrap_or(0);
            let sh = if size < 1024 {
                format!("{size} B")
            } else if size < 1_048_576 {
                format!("{:.1} KB", size as f64 / 1024.0)
            } else {
                format!("{:.1} MB", size as f64 / 1_048_576.0)
            };
            if let Some(obj) = a.as_object_mut() {
                obj.insert("size_human".into(), sh.into());
            }
            a
        })
        .collect();
    let json = serde_json::to_string(&enriched).unwrap_or_else(|_| "[]".into());
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json,
    )
        .into_response())
}

// ─── /mail/rules (legacy redirect) ───────────────────────────────────────────

async fn mail_rules_page(
    State(_st): State<AppState>,
    _headers: HeaderMap,
    _uri: Uri,
) -> WebResult<Response> {
    Ok(Redirect::to("/settings?tab=filters").into_response())
}

#[derive(Deserialize)]
struct SieveRulesForm {
    #[serde(default)]
    _script: String,
}

async fn mail_rules_save(
    State(_st): State<AppState>,
    _headers: HeaderMap,
    _uri: Uri,
    Form(_f): Form<SieveRulesForm>,
) -> WebResult<Response> {
    Ok(Redirect::to("/settings?tab=filters").into_response())
}

async fn mail_thread_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(tid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let folders = dedup_folders(
        get_json::<Vec<Folder>>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/folders",
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default(),
    );

    let enc_tid = utf8_percent_encode(&tid, NON_ALPHANUMERIC).to_string();
    let messages = get_json::<Vec<MessageListItem>>(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/threads/{enc_tid}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let subject = messages
        .iter()
        .find_map(|m| m.subject.clone())
        .unwrap_or_else(|| "(sem assunto)".into());

    Ok(askama_axum::IntoResponse::into_response(MailThreadTpl {
        me,
        folders,
        thread_id: tid,
        messages,
        subject,
    }))
}

// ─── /drive ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DriveQuery {
    parent_id: Option<String>,
}

/// Walk parent_id chain upward to build breadcrumb [(id, name), …] root-first.
/// Stops after 10 hops to avoid runaway loops on bad data.
async fn build_drive_breadcrumb(
    st: &AppState,
    headers: &HeaderMap,
    tenant: &str,
    user_id: &str,
    start_id: Option<&str>,
) -> Vec<(String, String)> {
    let Some(mut current_id) = start_id.map(str::to_owned) else {
        return vec![];
    };
    let mut crumbs: Vec<(String, String)> = Vec::new();
    for _ in 0..10 {
        let enc = utf8_percent_encode(&current_id, NON_ALPHANUMERIC).to_string();
        let path = format!("/api/v1/drive/files/{enc}");
        let Ok(Some(f)) = get_json::<DriveFile>(
            st,
            &st.backends.drive,
            &path,
            headers,
            Some((tenant, user_id)),
        )
        .await
        else {
            break;
        };
        crumbs.push((f.id.clone(), f.name.clone()));
        match f.parent_id {
            Some(pid) if !pid.is_empty() => current_id = pid,
            _ => break,
        }
    }
    crumbs.reverse();
    crumbs
}

async fn drive_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<DriveQuery>,
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
    let files =
        get_json::<Vec<DriveFile>>(&st, &st.backends.drive, &path, &headers, Some((&t, &u)))
            .await?
            .unwrap_or_default();
    let quota = get_json::<DriveQuota>(
        &st,
        &st.backends.drive,
        "/api/v1/drive/quota",
        &headers,
        Some((&t, &u)),
    )
    .await?;

    // Build breadcrumb by fetching each ancestor folder's metadata.
    let folder_ancestors =
        build_drive_breadcrumb(&st, &headers, &t, &u, q.parent_id.as_deref()).await;

    Ok(askama_axum::IntoResponse::into_response(DriveTpl {
        me,
        parent_id: q.parent_id,
        files,
        quota,
        folder_ancestors,
    }))
}

async fn drive_trash_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let files = get_json::<Vec<DriveFile>>(
        &st,
        &st.backends.drive,
        "/api/v1/drive/trash",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(DriveTrashTpl {
        me,
        files,
    }))
}

#[derive(Deserialize)]
struct UploadQuery {
    parent_id: Option<String>,
}

async fn drive_upload_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<UploadQuery>,
    body: Bytes,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let _ = crate::upstream::post_body(
        &st,
        &st.backends.drive,
        "/api/v1/drive/files",
        &headers,
        Some((&t, &u)),
        body,
        &ct,
    )
    .await?;
    let back = match &q.parent_id {
        Some(p) if !p.is_empty() => format!(
            "/drive?parent_id={}",
            utf8_percent_encode(p, NON_ALPHANUMERIC)
        ),
        _ => "/drive".into(),
    };
    Ok(Redirect::to(&back).into_response())
}

async fn drive_trash_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/drive").into_response())
}

async fn drive_restore_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::post_empty(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/restore"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/drive/trash").into_response())
}

async fn drive_purge_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}?permanent=true"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/drive/trash").into_response())
}

// ─── /calendar ───────────────────────────────────────────────────────────────

async fn calendar_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let calendars = get_json::<Vec<Calendar>>(
        &st,
        &st.backends.calendar,
        "/api/v1/calendars",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(CalendarTpl {
        me,
        calendars,
    }))
}

// ─── /contacts ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ContactsQuery {
    book_id: Option<String>,
}

async fn contacts_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<ContactsQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let books = get_json::<Vec<AddressBook>>(
        &st,
        &st.backends.contacts,
        "/api/v1/addressbooks",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let selected_book = q
        .book_id
        .clone()
        .or_else(|| books.first().map(|b| b.id.clone()));

    let contacts = if let Some(bid) = &selected_book {
        let enc = utf8_percent_encode(bid, NON_ALPHANUMERIC).to_string();
        get_json::<Vec<Contact>>(
            &st,
            &st.backends.contacts,
            &format!("/api/v1/addressbooks/{enc}/contacts"),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(askama_axum::IntoResponse::into_response(ContactsTpl {
        me,
        books,
        selected_book,
        contacts,
    }))
}

// ─── /mail/compose ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ComposeQuery {
    to: Option<String>,
    reply_to: Option<String>,
    forward: Option<String>,
    #[allow(dead_code)]
    folder: Option<String>,
}

async fn mail_compose_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<ComposeQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    // Pre-fill fields from reply/forward
    let (prefill_to, prefill_subject, prefill_body) =
        if let Some(ref orig_id) = q.reply_to.as_ref().or(q.forward.as_ref()) {
            match get_json::<MessageDetail>(
                &st,
                &st.backends.mail,
                &format!("/api/v1/mail/messages/{orig_id}"),
                &headers,
                Some((&t, &u)),
            )
            .await?
            {
                Some(orig) => {
                    let is_fwd = q.forward.is_some();
                    let to = if is_fwd {
                        String::new()
                    } else {
                        orig.from_addr.as_deref().unwrap_or("").to_string()
                    };
                    let subj_prefix = if is_fwd { "Fwd: " } else { "Re: " };
                    let subj = format!("{}{}", subj_prefix, orig.subject.as_deref().unwrap_or(""));
                    let body_prefix = format!(
                        "\n\n--- Mensagem original ---\nDe: {}\nData: {}\n\n{}",
                        orig.from_addr.as_deref().unwrap_or(""),
                        orig.date.as_deref().unwrap_or(""),
                        orig.body_text.as_deref().unwrap_or(""),
                    );
                    (to, subj, body_prefix)
                }
                None => (q.to.unwrap_or_default(), String::new(), String::new()),
            }
        } else {
            (q.to.unwrap_or_default(), String::new(), String::new())
        };

    // "Send as": owners who granted this user a SEND delegation. Resolve their
    // ids to emails so they can be picked as the From address.
    let send_as = {
        let to_me = get_json::<Vec<DelegationRaw>>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/delegations/to-me",
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        let mut out = Vec::new();
        for d in to_me
            .iter()
            .filter(|d| d.access.eq_ignore_ascii_case("SEND"))
        {
            out.push(resolve_email_by_id(&st, &d.owner_id, &headers, &t, &u).await);
        }
        out
    };

    Ok(MailComposeTpl {
        me,
        error: None,
        prefill_to,
        prefill_subject,
        prefill_body,
        send_as,
    }
    .into_response())
}

#[derive(Deserialize)]
struct ComposeForm {
    from: String,
    to: String,
    #[serde(default)]
    cc: String,
    subject: String,
    body_text: String,
    /// When set (a future ISO instant from the "send later" UI's `send_at`
    /// hidden field), schedule the message instead of sending immediately.
    #[serde(default)]
    send_at: Option<String>,
}

#[derive(serde::Serialize)]
struct SendPayload {
    from: String,
    to: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cc: Vec<String>,
    subject: String,
    body_text: String,
}

#[derive(serde::Serialize)]
struct SchedulePayload {
    from: String,
    to: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cc: Vec<String>,
    subject: String,
    body_text: String,
    deliver_at: String,
}

/// Normalize a browser `datetime-local` value ("YYYY-MM-DDTHH:MM") to RFC3339
/// with seconds + UTC offset, which the backend's rfc3339 deserializer needs.
/// Already-offset values pass through. Returns None on obviously-empty input.
fn to_rfc3339(local: &str) -> Option<String> {
    let s = local.trim();
    if s.is_empty() {
        return None;
    }
    // Has an offset or Z already → assume caller sent RFC3339.
    if s.ends_with('Z') || s[11.min(s.len())..].contains(['+', '-']) {
        return Some(s.to_string());
    }
    let with_secs = if s.len() == 16 {
        format!("{s}:00")
    } else {
        s.to_string()
    };
    Some(format!("{with_secs}Z"))
}

fn split_addrs(s: &str) -> Vec<String> {
    s.split([',', ';'])
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

async fn mail_compose_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<ComposeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let to = split_addrs(&f.to);
    if to.is_empty() {
        return Ok(MailComposeTpl {
            me,
            error: Some("Informe ao menos um destinatário.".into()),
            prefill_to: f.to.clone(),
            prefill_subject: f.subject.clone(),
            prefill_body: f.body_text.clone(),
            send_as: Vec::new(),
        }
        .into_response());
    }
    // Schedule for later when a (future) deliver_at is supplied; else send now.
    let cc = split_addrs(&f.cc);
    let deliver_at = f.send_at.as_deref().and_then(to_rfc3339);
    let status = if let Some(deliver_at) = deliver_at {
        crate::upstream::post_json(
            &st,
            &st.backends.mail,
            "/api/v1/mail/messages/schedule",
            &headers,
            Some((&t, &u)),
            &SchedulePayload {
                from: f.from,
                to,
                cc,
                subject: f.subject,
                body_text: f.body_text,
                deliver_at,
            },
        )
        .await?
    } else {
        crate::upstream::post_json(
            &st,
            &st.backends.mail,
            "/api/v1/mail/send",
            &headers,
            Some((&t, &u)),
            &SendPayload {
                from: f.from,
                to,
                cc,
                subject: f.subject,
                body_text: f.body_text,
            },
        )
        .await?
    };
    if (200..300).contains(&(status as u16)) {
        Ok(Redirect::to("/mail").into_response())
    } else {
        Ok(MailComposeTpl {
            me,
            error: Some(format!("Falha ao enviar (HTTP {status}).")),
            prefill_to: String::new(),
            prefill_subject: String::new(),
            prefill_body: String::new(),
            send_as: Vec::new(),
        }
        .into_response())
    }
}

// ─── /search (unified) ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UnifiedSearchQuery {
    q: Option<String>,
}

/// Per-source hit cap on the unified results page (keeps it scannable).
const UNIFIED_HITS_PER_SOURCE: usize = 6;

/// Pull up to `UNIFIED_HITS_PER_SOURCE` hits from a backend `/search` endpoint.
/// Responses differ per app, so we read them as JSON and pick a display string
/// (first present of `text_keys`) and build the item link via `href`. Failures
/// (service down, shape change) degrade to an empty group, never an error page.
async fn search_source(
    st: &AppState,
    backend: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: (&str, &str),
    text_keys: &[&str],
    href: impl Fn(&serde_json::Value) -> Option<String>,
) -> Vec<SearchHit> {
    let rows = get_json::<Vec<serde_json::Value>>(st, backend, path, headers, Some(ctx))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|r| {
            let text = text_keys
                .iter()
                .find_map(|k| r.get(*k).and_then(|v| v.as_str()))
                .unwrap_or("(sem título)")
                .chars()
                .take(80)
                .collect::<String>();
            href(&r).map(|h| SearchHit { text, href: h })
        })
        .take(UNIFIED_HITS_PER_SOURCE)
        .collect()
}

fn str_field(r: &serde_json::Value, key: &str) -> String {
    r.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// GET /search?q= — federated results across Mail, Drive, Agenda, Contatos, Chat.
/// Each source is queried via its own authenticated REST search; results are
/// grouped by app. The topbar's Enter action points here.
/// Federate the per-app searches into grouped results. `qt` is the trimmed,
/// non-empty query. Shared by the server-rendered `/search` page and the JSON
/// `/api/search` endpoint (the topbar dropdown). Returns groups in a fixed order.
async fn federate_search(
    st: &AppState,
    headers: &HeaderMap,
    ctx: (&str, &str),
    qt: &str,
) -> Vec<SearchGroup> {
    let enc = utf8_percent_encode(qt, NON_ALPHANUMERIC).to_string();
    // Bind each path so the borrowed &str outlives the federated futures.
    let p_mail = format!("/api/v1/mail/search?q={enc}&limit=6");
    let p_drive = format!("/api/v1/drive/search?q={enc}&limit=6");
    let p_cal = format!("/api/v1/calendars/events/search?q={enc}&limit=6");
    let p_con = format!("/api/v1/contacts/search?q={enc}&limit=6");
    let p_chat = format!("/api/v1/messages/search?q={enc}&limit=6");
    let p_notes = format!("/api/v1/notes/search?q={enc}&limit=6");

    let mail = search_source(
        st,
        &st.backends.mail,
        &p_mail,
        headers,
        ctx,
        &["subject", "preview_text", "from_addr"],
        |r| Some(format!("/mail/{}", str_field(r, "id"))),
    );
    let drive = search_source(
        st,
        &st.backends.drive,
        &p_drive,
        headers,
        ctx,
        &["name"],
        |_| Some("/drive".to_string()),
    );
    let cal = search_source(
        st,
        &st.backends.calendar,
        &p_cal,
        headers,
        ctx,
        &["summary", "title"],
        |r| {
            let c = str_field(r, "calendar_id");
            (!c.is_empty()).then(|| format!("/calendar/{c}"))
        },
    );
    let contacts = search_source(
        st,
        &st.backends.contacts,
        &p_con,
        headers,
        ctx,
        &["full_name", "email", "uid"],
        |_| Some("/contacts".to_string()),
    );
    let chat = search_source(
        st,
        &st.backends.chat,
        &p_chat,
        headers,
        ctx,
        &["body"],
        |r| {
            let cid = str_field(r, "channel_id");
            (!cid.is_empty()).then(|| format!("/chat/channels/{cid}"))
        },
    );
    let notes = search_source(
        st,
        &st.backends.notes,
        &p_notes,
        headers,
        ctx,
        &["title", "body"],
        |r| Some(format!("/notes?id={}", str_field(r, "id"))),
    );
    // Federate concurrently — one slow backend doesn't serialise the rest.
    let (mail, drive, cal, contacts, chat, notes) =
        tokio::join!(mail, drive, cal, contacts, chat, notes);

    vec![
        SearchGroup {
            label: "Mail".into(),
            icon: "✉".into(),
            hits: mail,
        },
        SearchGroup {
            label: "Drive".into(),
            icon: "📄".into(),
            hits: drive,
        },
        SearchGroup {
            label: "Agenda".into(),
            icon: "📅".into(),
            hits: cal,
        },
        SearchGroup {
            label: "Contatos".into(),
            icon: "👤".into(),
            hits: contacts,
        },
        SearchGroup {
            label: "Chat".into(),
            icon: "💬".into(),
            hits: chat,
        },
        SearchGroup {
            label: "Notas".into(),
            icon: "📝".into(),
            hits: notes,
        },
    ]
}

/// GET /api/search?q= — JSON for the topbar dropdown: a flat list of
/// `{cat, icon, text, href}` across all apps. Same federation as the /search
/// page; served by the web itself (unlike the dead /api/v1/* browser calls).
async fn unified_search_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(uq): Query<UnifiedSearchQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(axum::Json(serde_json::json!([])).into_response());
    };
    let (t, u) = ctx_of(&me);
    let query = uq.q.unwrap_or_default();
    let qt = query.trim();
    if qt.is_empty() {
        return Ok(axum::Json(serde_json::json!([])).into_response());
    }
    let groups = federate_search(&st, &headers, (t.as_str(), u.as_str()), qt).await;
    let items: Vec<serde_json::Value> = groups
        .iter()
        .flat_map(|g| {
            g.hits.iter().map(move |h| {
                serde_json::json!({ "cat": g.label, "icon": g.icon, "text": h.text, "href": h.href })
            })
        })
        .collect();
    Ok(axum::Json(items).into_response())
}

async fn unified_search_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(uq): Query<UnifiedSearchQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let query = uq.q.unwrap_or_default();
    let qt = query.trim();

    if qt.is_empty() {
        return Ok(askama_axum::IntoResponse::into_response(SearchTpl {
            me,
            query: String::new(),
            groups: Vec::new(),
            total: 0,
        }));
    }
    let groups = federate_search(&st, &headers, (t.as_str(), u.as_str()), qt).await;
    let total: usize = groups.iter().map(|g| g.hits.len()).sum();

    Ok(askama_axum::IntoResponse::into_response(SearchTpl {
        me,
        query,
        groups,
        total,
    }))
}

// ─── /mail/search ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MailSearchQuery {
    q: Option<String>,
    folder: Option<String>,
    from: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    has_attachment: Option<String>,
}

async fn mail_search_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<MailSearchQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let folders = dedup_folders(
        get_json::<Vec<Folder>>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/folders",
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default(),
    );

    let search_from = q.from.clone().unwrap_or_default();
    let search_folder = q.folder.clone().unwrap_or_default();
    let search_date_from = q.date_from.clone().unwrap_or_default();
    let search_date_to = q.date_to.clone().unwrap_or_default();
    let search_has_attach = q.has_attachment.as_deref() == Some("1");

    let (messages, query) = if let Some(ref qstr) = q.q {
        if !qstr.trim().is_empty() {
            let enc = utf8_percent_encode(qstr, NON_ALPHANUMERIC).to_string();
            let mut path = format!("/api/v1/mail/search?q={enc}");
            if !search_folder.is_empty() {
                path.push_str(&format!(
                    "&folder={}",
                    utf8_percent_encode(&search_folder, NON_ALPHANUMERIC)
                ));
            }
            if !search_from.is_empty() {
                path.push_str(&format!(
                    "&from={}",
                    utf8_percent_encode(&search_from, NON_ALPHANUMERIC)
                ));
            }
            if !search_date_from.is_empty() {
                path.push_str(&format!(
                    "&date_from={}",
                    utf8_percent_encode(&search_date_from, NON_ALPHANUMERIC)
                ));
            }
            if !search_date_to.is_empty() {
                path.push_str(&format!(
                    "&date_to={}",
                    utf8_percent_encode(&search_date_to, NON_ALPHANUMERIC)
                ));
            }
            if search_has_attach {
                path.push_str("&has_attachment=1");
            }
            let msgs = get_json::<Vec<MessageListItem>>(
                &st,
                &st.backends.mail,
                &path,
                &headers,
                Some((&t, &u)),
            )
            .await?
            .unwrap_or_default();
            (msgs, qstr.clone())
        } else {
            (vec![], String::new())
        }
    } else {
        (vec![], String::new())
    };

    Ok(askama_axum::IntoResponse::into_response(MailSearchTpl {
        me,
        folders,
        messages,
        query,
        search_from,
        search_folder,
        search_date_from,
        search_date_to,
        search_has_attachment: search_has_attach,
    }))
}

// ─── /mail/:id/attachments/:idx ──────────────────────────────────────────────

async fn mail_attachment_proxy(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, idx)): Path<(String, u32)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let path = format!("/api/v1/mail/messages/{id}/attachments/{idx}");
    let (status, ct, cd, body) =
        get_bytes(&st, &st.backends.mail, &path, &headers, Some((&t, &u))).await?;
    if !(200..300).contains(&(status as i32)) {
        return Ok((StatusCode::BAD_GATEWAY, "Anexo não encontrado").into_response());
    }
    let mut resp = axum::response::Response::new(axum::body::Body::from(body));
    if let Some(v) = ct {
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            v.parse()
                .unwrap_or(header::HeaderValue::from_static("application/octet-stream")),
        );
    }
    if let Some(v) = cd {
        if let Ok(hv) = v.parse() {
            resp.headers_mut().insert(header::CONTENT_DISPOSITION, hv);
        }
    }
    Ok(resp)
}

// ─── /mail/:id/flag ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FlagForm {
    flag: String,
    value: String,
    folder: Option<String>,
}

async fn mail_flag_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<FlagForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let set = f.value == "1" || f.value == "true";
    let payload = serde_json::json!({ "flag": f.flag, "set": set });
    let _ = patch_json(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{id}/flags"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    let folder = f.folder.unwrap_or_else(|| "INBOX".into());
    Ok(Redirect::to(&format!(
        "/mail/{}?folder={}",
        id,
        utf8_percent_encode(&folder, NON_ALPHANUMERIC)
    ))
    .into_response())
}

// ─── /mail/:id/move ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MoveForm {
    target_folder: String,
    from_folder: Option<String>,
}

async fn mail_move_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<MoveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "destination": f.target_folder });
    let _ = patch_json(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{id}/move"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    let back = f.from_folder.unwrap_or_else(|| "INBOX".into());
    Ok(Redirect::to(&format!(
        "/mail?folder={}",
        utf8_percent_encode(&back, NON_ALPHANUMERIC)
    ))
    .into_response())
}

// ─── /mail/:id/delete ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DeleteForm {
    folder: Option<String>,
}

async fn mail_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<DeleteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = delete_at(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{id}"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    let back = f.folder.unwrap_or_else(|| "INBOX".into());
    Ok(Redirect::to(&format!(
        "/mail?folder={}",
        utf8_percent_encode(&back, NON_ALPHANUMERIC)
    ))
    .into_response())
}

#[derive(Deserialize)]
struct SnoozeForm {
    /// RFC3339 instant to snooze until (computed client-side from a preset).
    snooze_until: String,
}

#[derive(serde::Serialize)]
struct SnoozePayload<'a> {
    snooze_until: &'a str,
}

/// POST /mail/:id/snooze — server-backed snooze: the backend hides the message
/// until `snooze_until`, then a waker returns it to the inbox. Returns the
/// upstream status so the JS can confirm or report failure.
async fn mail_snooze_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<SnoozeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let status = post_json(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{enc}/snooze"),
        &headers,
        Some((&t, &u)),
        &SnoozePayload {
            snooze_until: f.snooze_until.trim(),
        },
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

// ─── mail folder management ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct FolderCreateForm {
    name: String,
}

#[derive(serde::Serialize)]
struct FolderNamePayload<'a> {
    name: &'a str,
}

/// POST /mail/folders/create — create a user mail folder.
async fn mail_folder_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FolderCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let name = f.name.trim();
    if !name.is_empty() {
        let _ = post_json(
            &st,
            &st.backends.mail,
            "/api/v1/mail/folders",
            &headers,
            Some((&t, &u)),
            &FolderNamePayload { name },
        )
        .await?;
    }
    Ok(Redirect::to("/mail").into_response())
}

#[derive(Deserialize)]
struct FolderRenameForm {
    old_name: String,
    new_name: String,
}

/// POST /mail/folders/rename — rename a user folder (PATCH upstream by name).
async fn mail_folder_rename_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FolderRenameForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let new_name = f.new_name.trim();
    let enc_old = utf8_percent_encode(f.old_name.trim(), NON_ALPHANUMERIC).to_string();
    if !new_name.is_empty() {
        let _ = patch_json(
            &st,
            &st.backends.mail,
            &format!("/api/v1/mail/folders/{enc_old}"),
            &headers,
            Some((&t, &u)),
            &FolderNamePayload { name: new_name },
        )
        .await?;
    }
    Ok(Redirect::to(&format!(
        "/mail?folder={}",
        utf8_percent_encode(new_name, NON_ALPHANUMERIC)
    ))
    .into_response())
}

#[derive(Deserialize)]
struct FolderDeleteForm {
    name: String,
}

/// POST /mail/folders/delete — delete a user folder (DELETE upstream by name).
async fn mail_folder_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FolderDeleteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(f.name.trim(), NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/folders/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/mail").into_response())
}

#[derive(Deserialize)]
struct QuickReplyForm {
    reply_to: String,
    folder: Option<String>,
    body: String,
}

async fn mail_quick_reply_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<QuickReplyForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({
        "reply_to": f.reply_to,
        "body": f.body,
        "folder": f.folder.as_deref().unwrap_or("INBOX"),
    });
    let _ = post_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/quick-reply",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    let back = f.folder.unwrap_or_else(|| "INBOX".into());
    Ok(Redirect::to(&format!(
        "/mail?folder={}",
        utf8_percent_encode(&back, NON_ALPHANUMERIC)
    ))
    .into_response())
}

// ─── /drive extras ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct DriveSearchQuery {
    q: Option<String>,
}

async fn drive_search_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<DriveSearchQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let (files, _query) = if let Some(ref qstr) = q.q {
        if !qstr.trim().is_empty() {
            let payload = serde_json::json!({ "query": qstr });
            let results = match post_json(
                &st,
                &st.backends.drive,
                "/api/v1/drive/files/search",
                &headers,
                Some((&t, &u)),
                &payload,
            )
            .await
            {
                Ok(_) => get_json::<Vec<DriveFile>>(
                    &st,
                    &st.backends.drive,
                    &format!(
                        "/api/v1/drive/files/search?q={}",
                        utf8_percent_encode(qstr, NON_ALPHANUMERIC)
                    ),
                    &headers,
                    Some((&t, &u)),
                )
                .await?
                .unwrap_or_default(),
                Err(_) => vec![],
            };
            (results, qstr.clone())
        } else {
            (vec![], String::new())
        }
    } else {
        (vec![], String::new())
    };
    let quota = get_json::<DriveQuota>(
        &st,
        &st.backends.drive,
        "/api/v1/drive/quota",
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(askama_axum::IntoResponse::into_response(DriveTpl {
        me,
        parent_id: None,
        files,
        quota,
        folder_ancestors: vec![],
    }))
}

#[derive(Deserialize)]
struct MkdirForm {
    name: String,
    parent_id: Option<String>,
}

async fn drive_mkdir_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<MkdirForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "name": f.name, "parent_id": f.parent_id });
    let _ = post_json(
        &st,
        &st.backends.drive,
        "/api/v1/drive/files/mkdir",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    let back = match &f.parent_id {
        Some(p) if !p.is_empty() => format!(
            "/drive?parent_id={}",
            utf8_percent_encode(p, NON_ALPHANUMERIC)
        ),
        _ => "/drive".into(),
    };
    Ok(Redirect::to(&back).into_response())
}

#[derive(Deserialize)]
struct RenameForm {
    name: String,
    parent_id: Option<String>,
}

async fn drive_rename_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<RenameForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "name": f.name });
    let _ = patch_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    let back = match &f.parent_id {
        Some(p) if !p.is_empty() => format!(
            "/drive?parent_id={}",
            utf8_percent_encode(p, NON_ALPHANUMERIC)
        ),
        _ => "/drive".into(),
    };
    Ok(Redirect::to(&back).into_response())
}

#[derive(Deserialize)]
struct DriveMoveForm {
    target_parent_id: Option<String>,
    from_parent_id: Option<String>,
}

async fn drive_move_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<DriveMoveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "parent_id": f.target_parent_id });
    let _ = patch_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/move"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    let back = match &f.from_parent_id {
        Some(p) if !p.is_empty() => format!(
            "/drive?parent_id={}",
            utf8_percent_encode(p, NON_ALPHANUMERIC)
        ),
        _ => "/drive".into(),
    };
    Ok(Redirect::to(&back).into_response())
}

#[derive(Deserialize)]
struct BulkMoveForm {
    /// Comma-separated file/folder ids.
    ids: String,
    /// Destination folder id; blank → root.
    #[serde(default)]
    parent_id: String,
}

/// Parse the shared bulk-op form into (ids, parent_id) for the JSON payload.
fn bulk_payload(ids: &str, parent_id: &str) -> serde_json::Value {
    let ids: Vec<&str> = ids
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let parent = parent_id.trim();
    serde_json::json!({
        "ids": ids,
        "parent_id": if parent.is_empty() { None } else { Some(parent) },
    })
}

/// POST /drive/bulk-move — atomically move the selected items (backend caps 200).
async fn drive_bulk_move_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<BulkMoveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.drive,
        "/api/v1/drive/files/bulk-move",
        &headers,
        Some((&t, &u)),
        &bulk_payload(&f.ids, &f.parent_id),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /drive/bulk-copy — shallow-copy the selected items (backend caps 200).
async fn drive_bulk_copy_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<BulkMoveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.drive,
        "/api/v1/drive/files/bulk-copy",
        &headers,
        Some((&t, &u)),
        &bulk_payload(&f.ids, &f.parent_id),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// GET /drive/tags — overview of all tags with file counts.
async fn drive_tags_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let stats = get_json::<Vec<DriveTagStat>>(
        &st,
        &st.backends.drive,
        "/api/v1/drive/tags/stats",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(DriveTagsTpl {
        me,
        stats,
    }))
}

/// GET /drive/tags/:tag — files carrying a tag (full metadata).
async fn drive_tag_files_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(tag): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&tag, NON_ALPHANUMERIC).to_string();
    let files = get_json::<Vec<DriveFile>>(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/tags/{enc}/files"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(DriveTagFilesTpl {
        me,
        tag,
        files,
    }))
}

// ─── /contacts/gal ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GalQuery {
    q: Option<String>,
}

async fn contacts_gal_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<GalQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let contacts = if let Some(ref qstr) = q.q {
        if !qstr.trim().is_empty() {
            let enc = utf8_percent_encode(qstr, NON_ALPHANUMERIC).to_string();
            get_json::<Vec<GalContact>>(
                &st,
                &st.backends.contacts,
                &format!("/api/v1/gal/search?q={enc}"),
                &headers,
                Some((&t, &u)),
            )
            .await?
            .unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    let query = q.q.unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(
        crate::templates::GalSearchTpl {
            me,
            contacts,
            query,
        },
    ))
}

// ─── /api/gal/search (JSON autocomplete) ─────────────────────────────────────

#[allow(unused_variables)] // uri unused here; shared handler signature
async fn gal_search_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<GalQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok((StatusCode::UNAUTHORIZED, "[]").into_response());
    };
    let (t, u) = ctx_of(&me);
    let results = if let Some(ref qstr) = q.q {
        if !qstr.trim().is_empty() {
            let enc = utf8_percent_encode(qstr, NON_ALPHANUMERIC).to_string();
            get_json::<Vec<GalContact>>(
                &st,
                &st.backends.contacts,
                &format!("/api/v1/gal/search?q={enc}"),
                &headers,
                Some((&t, &u)),
            )
            .await?
            .unwrap_or_default()
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    let json = serde_json::to_string(&results).unwrap_or_else(|_| "[]".into());
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        json,
    )
        .into_response())
}

// ─── /drive/:id/share ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SharePageQuery {
    new_url: Option<String>,
    new_token: Option<String>,
}

async fn drive_share_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Query(q): Query<SharePageQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(f) => f,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let shares: Vec<ShareRow> = get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/shares"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(DriveShareTpl {
        me,
        file,
        shares,
        new_url: q.new_url,
        new_token: q.new_token,
    }
    .into_response())
}

#[derive(Deserialize)]
struct ShareCreateForm {
    ttl_hours: i64,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    max_downloads: Option<i32>,
}

#[derive(serde::Serialize)]
struct ShareCreatePayload {
    expires_in_seconds: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_downloads: Option<i32>,
}

#[derive(serde::Deserialize)]
struct ShareCreateResp {
    id: String,
    token: String,
    url: String,
}

async fn drive_share_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<ShareCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let ttl_s = f.ttl_hours.clamp(1, 720) * 3600;
    // Treat blank password / non-positive download cap as "unset".
    let password = f
        .password
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let max_downloads = f.max_downloads.filter(|&n| n > 0);
    let payload = ShareCreatePayload {
        expires_in_seconds: ttl_s,
        password,
        max_downloads,
    };
    // Precisamos do corpo de resposta → usa http client direto (não post_json que só retorna status).
    let url = format!(
        "{}/api/v1/drive/files/{}/shares",
        st.backends.drive.trim_end_matches('/'),
        id
    );
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
    let enc_url = utf8_percent_encode(&body.url, NON_ALPHANUMERIC).to_string();
    let enc_token = utf8_percent_encode(&body.token, NON_ALPHANUMERIC).to_string();
    Ok(Redirect::to(&format!(
        "/drive/{id}/share?new_url={enc_url}&new_token={enc_token}"
    ))
    .into_response())
}

async fn drive_share_revoke(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, sid)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = crate::upstream::delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/shares/{sid}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/drive/{id}/share")).into_response())
}

// ─── /drive/:id/versions ─────────────────────────────────────────────────────

async fn drive_versions_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(f) => f,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let versions: Vec<VersionRow> = get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/versions"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let tags: Vec<String> = get_json::<Vec<DriveFileTag>>(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/tags"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(|t| t.tag)
    .collect();
    Ok(DriveVersionsTpl {
        me,
        file,
        versions,
        tags,
    }
    .into_response())
}

async fn drive_version_restore(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, vno)): Path<(String, u32)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "version_no": vno });
    let _ = post_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/versions/{vno}/restore"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok(Redirect::to(&format!("/drive/{id}/versions")).into_response())
}

#[derive(Deserialize)]
struct DriveTagForm {
    tag: String,
}

#[derive(serde::Serialize)]
struct DriveTagPayload<'a> {
    tag: &'a str,
}

/// POST /drive/:id/tags/add — add a tag to a file.
async fn drive_tag_add_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<DriveTagForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tag = f.tag.trim();
    if !tag.is_empty() {
        let _ = post_json(
            &st,
            &st.backends.drive,
            &format!("/api/v1/drive/files/{id}/tags"),
            &headers,
            Some((&t, &u)),
            &DriveTagPayload { tag },
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/drive/{id}/versions")).into_response())
}

/// POST /drive/:id/tags/remove — remove a tag from a file.
async fn drive_tag_remove_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<DriveTagForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_tag = utf8_percent_encode(f.tag.trim(), NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/tags/{enc_tag}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/drive/{id}/versions")).into_response())
}

// ─── /drive/:id/edit — WOPI/Collabora iframe ─────────────────────────────────

async fn drive_edit_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };

    if !st.wopi.is_enabled() {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            "WOPI desabilitado — configure WOPI__SECRET no servidor",
        )
            .into_response());
    }

    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(f) => f,
        None => return Ok(login_redirect(&uri).into_response()),
    };

    if !file.is_editable() {
        return Ok((
            StatusCode::BAD_REQUEST,
            "Arquivo não suportado pelo editor (mime não editável).",
        )
            .into_response());
    }

    let token = crate::wopi::sign_token(
        st.wopi.secret.as_bytes(),
        &file.id,
        &me.tenant_id,
        &me.user_id,
        st.wopi.token_ttl_secs,
    );
    let iframe_url =
        crate::wopi::build_iframe_url(&st.wopi.collabora_url, &st.wopi.drive_url, &file.id, &token);

    Ok(DriveEditTpl {
        me,
        file,
        iframe_url,
    }
    .into_response())
}

async fn drive_preview_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let file: DriveFile = match get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(f) => f,
        None => return Ok(StatusCode::NOT_FOUND.into_response()),
    };
    if !file.is_previewable() {
        return Ok((
            StatusCode::BAD_REQUEST,
            "Pré-visualização não disponível para este tipo de arquivo.",
        )
            .into_response());
    }
    let download_url = format!(
        "{}/api/v1/drive/files/{id}",
        st.backends.drive.trim_end_matches('/')
    );
    Ok(askama_axum::IntoResponse::into_response(DrivePreviewTpl {
        me,
        file,
        download_url,
    }))
}

// ─── /calendar/:cal_id — month grid ─────────────────────────────────────────

use time::{macros::format_description, Date, Month, OffsetDateTime};

#[derive(Deserialize)]
struct MonthQuery {
    month: Option<String>,
}

/// Parse "YYYY-MM" → (year, month). Fallback: today.
fn parse_ym(s: Option<&str>) -> (i32, u8) {
    let today = OffsetDateTime::now_utc().date();
    let fallback = (today.year(), today.month() as u8);
    let Some(raw) = s else { return fallback };
    let parts: Vec<&str> = raw.split('-').collect();
    if parts.len() != 2 {
        return fallback;
    }
    let (Ok(y), Ok(m)) = (parts[0].parse::<i32>(), parts[1].parse::<u8>()) else {
        return fallback;
    };
    if !(1..=12).contains(&m) {
        return fallback;
    }
    (y, m)
}

fn month_label_pt(m: u8) -> &'static str {
    [
        "jan", "fev", "mar", "abr", "mai", "jun", "jul", "ago", "set", "out", "nov", "dez",
    ][(m as usize).saturating_sub(1).min(11)]
}

fn u8_to_month(m: u8) -> Month {
    match m {
        1 => Month::January,
        2 => Month::February,
        3 => Month::March,
        4 => Month::April,
        5 => Month::May,
        6 => Month::June,
        7 => Month::July,
        8 => Month::August,
        9 => Month::September,
        10 => Month::October,
        11 => Month::November,
        _ => Month::December,
    }
}

/// Add one month, wrapping year boundary.
fn next_ym(y: i32, m: u8) -> (i32, u8) {
    if m == 12 {
        (y + 1, 1)
    } else {
        (y, m + 1)
    }
}
fn prev_ym(y: i32, m: u8) -> (i32, u8) {
    if m == 1 {
        (y - 1, 12)
    } else {
        (y, m - 1)
    }
}

/// Build 6×7 cell grid — Monday-first.
fn build_weeks(
    year: i32,
    month: u8,
    events_by_day: &std::collections::HashMap<String, Vec<Event>>,
) -> Vec<Vec<MonthCell>> {
    let today = OffsetDateTime::now_utc().date();
    let first = Date::from_calendar_date(year, u8_to_month(month), 1).unwrap();
    let lead = first.weekday().number_from_monday() as i32 - 1;
    let start = first - time::Duration::days(lead as i64);
    (0..6)
        .map(|w| {
            (0..7)
                .map(|d| {
                    let offset = w * 7 + d;
                    let day = start + time::Duration::days(offset as i64);
                    let iso = day
                        .format(format_description!("[year]-[month]-[day]"))
                        .unwrap();
                    let in_month = day.month() as u8 == month && day.year() == year;
                    let events = events_by_day.get(&iso).cloned().unwrap_or_default();
                    MonthCell {
                        iso: iso.clone(),
                        day: day.day(),
                        in_month,
                        is_today: day == today,
                        events,
                    }
                })
                .collect()
        })
        .collect()
}

async fn calendar_month_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    Query(q): Query<MonthQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let calendars = get_json::<Vec<Calendar>>(
        &st,
        &st.backends.calendar,
        "/api/v1/calendars",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let Some(selected) = calendars.iter().find(|c| c.id == cal_id).cloned() else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };

    let (y, m) = parse_ym(q.month.as_deref());

    // range = first day of month → first day of next month (UTC)
    let first = Date::from_calendar_date(y, u8_to_month(m), 1).unwrap();
    let (ny, nm) = next_ym(y, m);
    let next_first = Date::from_calendar_date(ny, u8_to_month(nm), 1).unwrap();
    let from = first
        .format(format_description!("[year]-[month]-[day]T00:00:00Z"))
        .unwrap();
    let to = next_first
        .format(format_description!("[year]-[month]-[day]T00:00:00Z"))
        .unwrap();

    let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let events = get_json::<Vec<Event>>(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events?from={from}&to={to}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let mut by_day: std::collections::HashMap<String, Vec<Event>> =
        std::collections::HashMap::new();
    for ev in events {
        let key = ev.date_key();
        if !key.is_empty() {
            by_day.entry(key).or_default().push(ev);
        }
    }
    for v in by_day.values_mut() {
        v.sort_by(|a, b| a.dtstart.cmp(&b.dtstart));
    }

    let weeks = build_weeks(y, m, &by_day);

    let (py, pm) = prev_ym(y, m);
    let (ny2, nm2) = (ny, nm);
    let prev_link = format!("/calendar/{cal_id}?month={py:04}-{pm:02}");
    let next_link = format!("/calendar/{cal_id}?month={ny2:04}-{nm2:02}");
    let today_link = format!("/calendar/{cal_id}");
    let month_label = format!("{} {:04}", month_label_pt(m), y);

    Ok(askama_axum::IntoResponse::into_response(CalendarMonthTpl {
        me,
        calendars,
        selected,
        year: y,
        month: m,
        month_label,
        prev_link,
        next_link,
        today_link,
        weekday_labels: vec!["Seg", "Ter", "Qua", "Qui", "Sex", "Sáb", "Dom"],
        weeks,
    }))
}

// ─── week / day views ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WeekQuery {
    start: Option<String>,
}

#[derive(Deserialize)]
struct DayQuery {
    date: Option<String>,
}

fn parse_iso_date(s: &str) -> Option<Date> {
    let bytes = s.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let y: i32 = s[0..4].parse().ok()?;
    let m: u8 = s[5..7].parse().ok()?;
    let d: u8 = s[8..10].parse().ok()?;
    Date::from_calendar_date(y, u8_to_month(m), d).ok()
}

fn weekday_pt(d: Date) -> &'static str {
    use time::Weekday::*;
    match d.weekday() {
        Monday => "Seg",
        Tuesday => "Ter",
        Wednesday => "Qua",
        Thursday => "Qui",
        Friday => "Sex",
        Saturday => "Sáb",
        Sunday => "Dom",
    }
}

fn month_label_short(d: Date) -> String {
    format!("{:02}/{:02}", d.day(), d.month() as u8)
}

/// Fetch events from backend within [from, to).
async fn fetch_events(
    st: &AppState,
    headers: &HeaderMap,
    t: &str,
    u: &str,
    cal_id: &str,
    from: &str,
    to: &str,
) -> WebResult<Vec<Event>> {
    let enc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC).to_string();
    Ok(get_json::<Vec<Event>>(
        st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events?from={from}&to={to}"),
        headers,
        Some((t, u)),
    )
    .await?
    .unwrap_or_default())
}

async fn calendar_week_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    Query(q): Query<WeekQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let calendars = get_json::<Vec<Calendar>>(
        &st,
        &st.backends.calendar,
        "/api/v1/calendars",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let Some(selected) = calendars.iter().find(|c| c.id == cal_id).cloned() else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };

    let today = OffsetDateTime::now_utc().date();
    let base = q.start.as_deref().and_then(parse_iso_date).unwrap_or(today);
    // Monday-first: back up (weekday-1) days.
    let lead = base.weekday().number_from_monday() as i64 - 1;
    let mon = base - time::Duration::days(lead);
    let sun = mon + time::Duration::days(6);

    let from = mon
        .format(format_description!("[year]-[month]-[day]T00:00:00Z"))
        .unwrap();
    let to_d = mon + time::Duration::days(7);
    let to = to_d
        .format(format_description!("[year]-[month]-[day]T00:00:00Z"))
        .unwrap();

    let mut events = fetch_events(&st, &headers, &t, &u, &cal_id, &from, &to).await?;
    events.sort_by(|a, b| a.dtstart.cmp(&b.dtstart));

    let mut by_day: std::collections::HashMap<String, Vec<Event>> =
        std::collections::HashMap::new();
    for ev in events {
        let key = ev.date_key();
        if !key.is_empty() {
            by_day.entry(key).or_default().push(ev);
        }
    }

    let days: Vec<DayColumn> = (0..7)
        .map(|i| {
            let d = mon + time::Duration::days(i);
            let iso = d
                .format(format_description!("[year]-[month]-[day]"))
                .unwrap();
            let label = format!("{} {}", weekday_pt(d), month_label_short(d));
            DayColumn {
                events: by_day.remove(&iso).unwrap_or_default(),
                is_today: d == today,
                date_iso: iso,
                label,
            }
        })
        .collect();

    let prev = mon - time::Duration::days(7);
    let next = mon + time::Duration::days(7);
    let prev_link = format!(
        "/calendar/{cal_id}/week?start={}",
        prev.format(format_description!("[year]-[month]-[day]"))
            .unwrap()
    );
    let next_link = format!(
        "/calendar/{cal_id}/week?start={}",
        next.format(format_description!("[year]-[month]-[day]"))
            .unwrap()
    );
    let today_link = format!("/calendar/{cal_id}/week");
    let month_link = format!(
        "/calendar/{cal_id}?month={}-{:02}",
        mon.year(),
        mon.month() as u8
    );
    let day_link = format!(
        "/calendar/{cal_id}/day?date={}",
        today
            .format(format_description!("[year]-[month]-[day]"))
            .unwrap()
    );
    let week_label = format!(
        "{} – {}",
        mon.format(format_description!("[day]/[month]")).unwrap(),
        sun.format(format_description!("[day]/[month]/[year]"))
            .unwrap()
    );

    Ok(askama_axum::IntoResponse::into_response(CalendarWeekTpl {
        me,
        calendars,
        selected,
        week_label,
        prev_link,
        next_link,
        today_link,
        month_link,
        day_link,
        days,
    }))
}

async fn calendar_day_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    Query(q): Query<DayQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);

    let calendars = get_json::<Vec<Calendar>>(
        &st,
        &st.backends.calendar,
        "/api/v1/calendars",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let Some(selected) = calendars.iter().find(|c| c.id == cal_id).cloned() else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };

    let today = OffsetDateTime::now_utc().date();
    let d = q.date.as_deref().and_then(parse_iso_date).unwrap_or(today);
    let d_next = d + time::Duration::days(1);

    let from = d
        .format(format_description!("[year]-[month]-[day]T00:00:00Z"))
        .unwrap();
    let to = d_next
        .format(format_description!("[year]-[month]-[day]T00:00:00Z"))
        .unwrap();
    let mut events = fetch_events(&st, &headers, &t, &u, &cal_id, &from, &to).await?;
    events.sort_by(|a, b| a.dtstart.cmp(&b.dtstart));

    let date_iso = d
        .format(format_description!("[year]-[month]-[day]"))
        .unwrap();
    let prev = d - time::Duration::days(1);
    let next = d + time::Duration::days(1);
    let iso_fmt = format_description!("[year]-[month]-[day]");
    let prev_link = format!(
        "/calendar/{cal_id}/day?date={}",
        prev.format(iso_fmt).unwrap()
    );
    let next_link = format!(
        "/calendar/{cal_id}/day?date={}",
        next.format(iso_fmt).unwrap()
    );
    let today_link = format!("/calendar/{cal_id}/day");
    let week_link = format!(
        "/calendar/{cal_id}/week?start={}",
        d.format(iso_fmt).unwrap()
    );
    let month_link = format!(
        "/calendar/{cal_id}?month={}-{:02}",
        d.year(),
        d.month() as u8
    );
    let date_label = format!(
        "{}, {:02}/{:02}/{:04}",
        weekday_pt(d),
        d.day(),
        d.month() as u8,
        d.year()
    );

    Ok(askama_axum::IntoResponse::into_response(CalendarDayTpl {
        me,
        calendars,
        selected,
        date_label,
        date_iso,
        prev_link,
        next_link,
        today_link,
        week_link,
        month_link,
        events,
        hours: (0u8..24).collect(),
    }))
}

// ─── event create/edit/delete ───────────────────────────────────────────────

#[derive(Deserialize)]
struct EventForm {
    summary: String,
    #[serde(default)]
    location: String,
    #[serde(default)]
    description: String,
    dtstart: String, // "YYYY-MM-DDTHH:MM"
    dtend: String,
    #[serde(default)]
    attendees: String, // newline / comma / semicolon separated
    /// Reminder lead times in minutes before start, comma-separated (e.g.
    /// "15,60"). Each becomes a VALARM with a relative `-PT{m}M` trigger.
    #[serde(default)]
    reminders: String,
}

#[derive(Deserialize, Default)]
struct AttendeeRow {
    email: String,
    #[serde(default)]
    partstat: Option<String>,
}

#[derive(Deserialize)]
struct NewQuery {
    date: Option<String>,
}

async fn event_new_form(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    Query(q): Query<NewQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let calendars = get_json::<Vec<Calendar>>(
        &st,
        &st.backends.calendar,
        "/api/v1/calendars",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let Some(calendar) = calendars.into_iter().find(|c| c.id == cal_id) else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };
    let date = q.date.unwrap_or_else(|| {
        OffsetDateTime::now_utc()
            .date()
            .format(format_description!("[year]-[month]-[day]"))
            .unwrap()
    });
    Ok(EventFormTpl {
        me,
        calendar,
        event_id: None,
        summary: String::new(),
        location: String::new(),
        description: String::new(),
        dtstart: format!("{date}T09:00"),
        dtend: format!("{date}T10:00"),
        attendees: String::new(),
        attendee_pills: Vec::new(),
        reminders: String::new(),
        error: None,
    }
    .into_response())
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
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

fn parse_attendees(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('@'))
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Parse a comma-separated minutes-before list ("15,60") into VALARM blocks
/// with a DISPLAY action and a `-PT{m}M` relative trigger. Skips blanks,
/// non-numbers, and negatives; dedups; caps at 10 to bound the payload.
fn build_valarms(reminders: &str, summary: &str) -> String {
    let mut mins: Vec<u32> = reminders
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();
    mins.sort_unstable();
    mins.dedup();
    mins.truncate(10);
    let mut out = String::new();
    for m in mins {
        out.push_str("BEGIN:VALARM\r\n");
        out.push_str("ACTION:DISPLAY\r\n");
        out.push_str(&format!("TRIGGER:-PT{m}M\r\n"));
        out.push_str(&format!("DESCRIPTION:{}\r\n", escape_ical(summary)));
        out.push_str("END:VALARM\r\n");
    }
    out
}

/// Extract reminder lead times (minutes) from an event's iCalendar by reading
/// each VALARM's `TRIGGER:-PT{n}M` / `-PT{n}H` / `-P{n}D`. Returns a sorted,
/// deduped, comma-separated minutes list to seed the edit form. Best-effort:
/// triggers it can't parse as a simple negative duration are skipped.
fn valarm_minutes(ical: &str) -> String {
    let mut mins: Vec<u32> = ical
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l
                .strip_prefix("TRIGGER:-P")
                .or_else(|| l.strip_prefix("TRIGGER:-p"))?;
            parse_neg_duration_minutes(rest)
        })
        .collect();
    mins.sort_unstable();
    mins.dedup();
    mins.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse the body of a negative iCal duration after the leading `-P`
/// (e.g. "T15M", "T1H", "1D") into total minutes. Returns None on anything
/// more exotic than a single D/H/M component.
fn parse_neg_duration_minutes(body: &str) -> Option<u32> {
    let b = body.trim().to_ascii_uppercase();
    if let Some(d) = b.strip_prefix('T') {
        // Time component: NNh or NNm.
        if let Some(h) = d.strip_suffix('H') {
            return h.parse::<u32>().ok().map(|n| n * 60);
        }
        if let Some(m) = d.strip_suffix('M') {
            return m.parse::<u32>().ok();
        }
        None
    } else if let Some(days) = b.strip_suffix('D') {
        days.parse::<u32>().ok().map(|n| n * 24 * 60)
    } else {
        None
    }
}

fn build_vcalendar(
    uid: &str,
    organizer_email: Option<&str>,
    attendees: &[String],
    method: Option<&str>,
    f: &EventForm,
) -> Option<String> {
    let dtstart = local_to_ical_utc(&f.dtstart)?;
    let dtend = local_to_ical_utc(&f.dtend)?;
    let now = OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]T[hour][minute][second]Z"
        ))
        .ok()?;
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
        ical.push_str(&format!(
            "DESCRIPTION:{}\r\n",
            escape_ical(f.description.trim())
        ));
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
    // VALARMs carry reminders into the stored event (and to CalDAV clients).
    // A cancellation does not need them.
    if method != Some("CANCEL") {
        ical.push_str(&build_valarms(&f.reminders, f.summary.trim()));
    }
    ical.push_str("END:VEVENT\r\n");
    ical.push_str("END:VCALENDAR\r\n");
    Some(ical)
}

async fn event_new_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    Form(f): Form<EventForm>,
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
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events"),
        &headers,
        Some((&t, &u)),
        Bytes::from(ical),
        "text/calendar; charset=utf-8",
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    if !attendees.is_empty() {
        let Some(itip) = build_vcalendar(&uid, Some(&me.email), &attendees, Some("REQUEST"), &f)
        else {
            return Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response());
        };
        let _ = post_body(
            &st,
            &st.backends.calendar,
            "/api/v1/scheduling/send",
            &headers,
            Some((&t, &u)),
            Bytes::from(itip),
            "text/calendar; charset=utf-8",
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response())
}

async fn event_edit_form(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cal_id, id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let calendars = get_json::<Vec<Calendar>>(
        &st,
        &st.backends.calendar,
        "/api/v1/calendars",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let Some(calendar) = calendars.into_iter().find(|c| c.id == cal_id) else {
        return Ok((StatusCode::NOT_FOUND, "Calendário não encontrado").into_response());
    };
    let enc_c = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_e = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let event: Event = match get_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(e) => e,
        None => return Ok((StatusCode::NOT_FOUND, "Evento não encontrado").into_response()),
    };
    fn iso_to_local(s: &str) -> String {
        // "2026-05-01T10:00:00+00:00" → "2026-05-01T10:00"
        if s.len() >= 16 {
            s[..16].to_string()
        } else {
            s.to_string()
        }
    }
    let atts: Vec<AttendeeRow> = get_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}/attendees"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let attendees_text = atts
        .iter()
        .map(|a| a.email.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let attendee_pills = atts
        .iter()
        .map(|a| crate::templates::AttendeePill {
            email: a.email.clone(),
            partstat: a
                .partstat
                .clone()
                .unwrap_or_else(|| "NEEDS-ACTION".into())
                .to_ascii_uppercase(),
        })
        .collect();
    Ok(EventFormTpl {
        me,
        calendar,
        event_id: Some(id),
        summary: event.summary.unwrap_or_default(),
        location: event.location.unwrap_or_default(),
        description: event.description.unwrap_or_default(),
        dtstart: event
            .dtstart
            .as_deref()
            .map(iso_to_local)
            .unwrap_or_default(),
        dtend: event.dtend.as_deref().map(iso_to_local).unwrap_or_default(),
        attendees: attendees_text,
        attendee_pills,
        reminders: event
            .ical_raw
            .as_deref()
            .map(valarm_minutes)
            .unwrap_or_default(),
        error: None,
    }
    .into_response())
}

async fn event_edit_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cal_id, id)): Path<(String, String)>,
    Form(f): Form<EventForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    // fetch existing UID → preserve for replace
    let enc_c = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_e = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let existing: Event = match get_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(e) => e,
        None => return Ok((StatusCode::NOT_FOUND, "Evento não encontrado").into_response()),
    };
    let attendees = parse_attendees(&f.attendees);
    let organizer = existing
        .organizer_email
        .as_deref()
        .or(Some(me.email.as_str()));
    let Some(ical) = build_vcalendar(&existing.uid, organizer, &attendees, None, &f) else {
        return Ok((StatusCode::BAD_REQUEST, "Datas inválidas").into_response());
    };
    let status = put_body(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers,
        Some((&t, &u)),
        Bytes::from(ical),
        "text/calendar; charset=utf-8",
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    if !attendees.is_empty() {
        let Some(itip) = build_vcalendar(&existing.uid, organizer, &attendees, Some("REQUEST"), &f)
        else {
            return Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response());
        };
        let _ = post_body(
            &st,
            &st.backends.calendar,
            "/api/v1/scheduling/send",
            &headers,
            Some((&t, &u)),
            Bytes::from(itip),
            "text/calendar; charset=utf-8",
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response())
}

async fn event_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
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
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    let atts_pre: Vec<AttendeeRow> = get_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}/attendees"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let _ = delete_at(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;

    // After delete: if organizer is current user (or unset) AND there are attendees,
    // dispatch METHOD:CANCEL to all attendees so their clients withdraw.
    if let Some(ev) = event_pre {
        let is_organizer = ev
            .organizer_email
            .as_deref()
            .map(|o| o.eq_ignore_ascii_case(&me.email))
            .unwrap_or(true);
        let attendee_emails: Vec<String> = atts_pre
            .into_iter()
            .map(|a| a.email)
            .filter(|e| !e.is_empty())
            .collect();
        if is_organizer && !attendee_emails.is_empty() {
            let f = EventForm {
                summary: ev.summary.clone().unwrap_or_else(|| "(cancelado)".into()),
                location: ev.location.clone().unwrap_or_default(),
                description: ev.description.clone().unwrap_or_default(),
                // dtstart/dtend back to "YYYY-MM-DDTHH:MM" so build_vcalendar can re-encode
                dtstart: ev
                    .dtstart
                    .as_deref()
                    .map(|s| s.get(0..16).unwrap_or("").to_string())
                    .unwrap_or_default(),
                dtend: ev
                    .dtend
                    .as_deref()
                    .map(|s| s.get(0..16).unwrap_or("").to_string())
                    .unwrap_or_default(),
                attendees: String::new(),
                reminders: String::new(),
            };
            let organizer = ev.organizer_email.as_deref().or(Some(me.email.as_str()));
            if let Some(itip) =
                build_vcalendar(&ev.uid, organizer, &attendee_emails, Some("CANCEL"), &f)
            {
                let _ = post_body(
                    &st,
                    &st.backends.calendar,
                    "/api/v1/scheduling/send",
                    &headers,
                    Some((&t, &u)),
                    Bytes::from(itip),
                    "text/calendar; charset=utf-8",
                )
                .await?;
            }
        }
    }

    Ok(Redirect::to(&format!("/calendar/{cal_id}")).into_response())
}

// ─── RSVP action ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RsvpForm {
    partstat: String,
}

async fn event_rsvp_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cal_id, id)): Path<(String, String)>,
    Form(f): Form<RsvpForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_c = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_e = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let body = serde_json::json!({"partstat": f.partstat});
    let _ = post_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}/rsvp"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await;
    Ok(Redirect::to(&format!("/calendar/{cal_id}/events/{id}/edit")).into_response())
}

/// Unique-enough UID for iCal VEVENTs — unix nanos as 32-hex.
fn uuid_v4_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:032x}", nanos)
}

// ─── contacts CRUD ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ContactForm {
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    given_name: String,
    #[serde(default)]
    family_name: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    phone: String,
    #[serde(default)]
    organization: String,
}

fn escape_vcard(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

fn build_vcard(uid: &str, f: &ContactForm) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCARD\r\n");
    out.push_str("VERSION:4.0\r\n");
    out.push_str(&format!("UID:{uid}\r\n"));
    // N: family;given;;; ;   FN: full_name (fallback to join)
    let family = escape_vcard(f.family_name.trim());
    let given = escape_vcard(f.given_name.trim());
    out.push_str(&format!("N:{family};{given};;;\r\n"));
    let fn_value = if f.full_name.trim().is_empty() {
        format!("{} {}", f.given_name.trim(), f.family_name.trim())
            .trim()
            .to_string()
    } else {
        f.full_name.trim().to_string()
    };
    if !fn_value.is_empty() {
        out.push_str(&format!("FN:{}\r\n", escape_vcard(&fn_value)));
    }
    if !f.email.trim().is_empty() {
        out.push_str(&format!(
            "EMAIL;TYPE=INTERNET:{}\r\n",
            escape_vcard(f.email.trim())
        ));
    }
    if !f.phone.trim().is_empty() {
        out.push_str(&format!(
            "TEL;TYPE=VOICE:{}\r\n",
            escape_vcard(f.phone.trim())
        ));
    }
    if !f.organization.trim().is_empty() {
        out.push_str(&format!("ORG:{}\r\n", escape_vcard(f.organization.trim())));
    }
    out.push_str("END:VCARD\r\n");
    out
}

async fn load_book(
    st: &AppState,
    headers: &HeaderMap,
    t: &str,
    u: &str,
    book_id: &str,
) -> WebResult<Option<AddressBook>> {
    let books = get_json::<Vec<AddressBook>>(
        st,
        &st.backends.contacts,
        "/api/v1/addressbooks",
        headers,
        Some((t, u)),
    )
    .await?
    .unwrap_or_default();
    Ok(books.into_iter().find(|b| b.id == book_id))
}

async fn contact_new_form(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
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
        me,
        book,
        contact_id: None,
        full_name: String::new(),
        given_name: String::new(),
        family_name: String::new(),
        email: String::new(),
        phone: String::new(),
        organization: String::new(),
        error: None,
    }
    .into_response())
}

async fn contact_new_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(book_id): Path<String>,
    Form(f): Form<ContactForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let uid = format!("web-{}", uuid_v4_hex());
    let vcard = build_vcard(&uid, &f);
    let enc = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let status = post_body(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}/contacts"),
        &headers,
        Some((&t, &u)),
        Bytes::from(vcard),
        "text/vcard; charset=utf-8",
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    let enc2 = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    Ok(Redirect::to(&format!("/contacts?book_id={enc2}")).into_response())
}

async fn contact_edit_form(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
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
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    else {
        return Ok((StatusCode::NOT_FOUND, "Contato não encontrado").into_response());
    };
    Ok(ContactFormTpl {
        me,
        book,
        contact_id: Some(id),
        full_name: contact.full_name.unwrap_or_default(),
        given_name: contact.given_name.unwrap_or_default(),
        family_name: contact.family_name.unwrap_or_default(),
        email: contact.email.unwrap_or_default(),
        phone: contact.phone.unwrap_or_default(),
        organization: contact.organization.unwrap_or_default(),
        error: None,
    }
    .into_response())
}

async fn contact_edit_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((book_id, id)): Path<(String, String)>,
    Form(f): Form<ContactForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let Some(existing): Option<Contact> = get_json(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    else {
        return Ok((StatusCode::NOT_FOUND, "Contato não encontrado").into_response());
    };
    let uid = existing
        .uid
        .clone()
        .unwrap_or_else(|| format!("web-{}", uuid_v4_hex()));
    let vcard = build_vcard(&uid, &f);
    let status = put_body(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers,
        Some((&t, &u)),
        Bytes::from(vcard),
        "text/vcard; charset=utf-8",
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    Ok(Redirect::to(&format!("/contacts?book_id={enc_b}")).into_response())
}

async fn contact_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((book_id, id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/contacts?book_id={enc_b}")).into_response())
}

async fn contacts_export_vcf(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(book_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let url = format!(
        "{}/api/v1/addressbooks/{enc}/contacts.vcf",
        st.backends.contacts.trim_end_matches('/')
    );
    let mut req = st.http.get(&url);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    match req.send().await {
        Ok(r) if r.status().is_success() => {
            let body = r.bytes().await.unwrap_or_default();
            Ok((
                [
                    (header::CONTENT_TYPE, "text/vcard; charset=utf-8"),
                    (
                        header::CONTENT_DISPOSITION,
                        "attachment; filename=\"contacts.vcf\"",
                    ),
                ],
                body,
            )
                .into_response())
        }
        _ => Ok((StatusCode::BAD_GATEWAY, "Falha ao exportar contatos.").into_response()),
    }
}

async fn contacts_import_vcf(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(book_id): Path<String>,
    body: Bytes,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/vcard")
        .to_string();
    let _ = crate::upstream::post_body(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}/import"),
        &headers,
        Some((&t, &u)),
        body,
        &ct,
    )
    .await?;
    Ok(Redirect::to(&format!("/contacts?book_id={enc}")).into_response())
}

// ─── contact groups (distribution lists) ─────────────────────────────────────

/// GET /contacts/groups — list the caller's server-backed contact groups.
async fn contact_groups_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let groups = get_json::<Vec<ContactGroup>>(
        &st,
        &st.backends.contacts,
        "/api/v1/contact-groups",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(ContactGroupsTpl {
        me,
        groups,
    }))
}

#[derive(Deserialize)]
struct GroupForm {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(serde::Serialize)]
struct NewGroupPayload<'a> {
    name: &'a str,
    description: Option<&'a str>,
}

/// POST /contacts/groups — create a group.
async fn contact_group_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    axum::Form(f): axum::Form<GroupForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let name = f.name.trim();
    if name.is_empty() {
        return Ok(Redirect::to("/contacts/groups").into_response());
    }
    let desc = f
        .description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let _ = post_json(
        &st,
        &st.backends.contacts,
        "/api/v1/contact-groups",
        &headers,
        Some((&t, &u)),
        &NewGroupPayload {
            name,
            description: desc,
        },
    )
    .await?;
    Ok(Redirect::to("/contacts/groups").into_response())
}

/// GET /contacts/groups/:id — group members + candidates to add.
async fn contact_group_detail_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();

    let group = match get_json::<ContactGroup>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/contact-groups/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(g) => g,
        None => return Ok(Redirect::to("/contacts/groups").into_response()),
    };

    let members = get_json::<Vec<Contact>>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/contact-groups/{enc}/members"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    // Candidates = default address book contacts not already members.
    let books = get_json::<Vec<AddressBook>>(
        &st,
        &st.backends.contacts,
        "/api/v1/addressbooks",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let candidates = if let Some(book) = books.iter().find(|b| b.is_default).or(books.first()) {
        let benc = utf8_percent_encode(&book.id, NON_ALPHANUMERIC).to_string();
        let all = get_json::<Vec<Contact>>(
            &st,
            &st.backends.contacts,
            &format!("/api/v1/addressbooks/{benc}/contacts"),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        let member_ids: std::collections::HashSet<&str> =
            members.iter().map(|m| m.id.as_str()).collect();
        all.into_iter()
            .filter(|c| !member_ids.contains(c.id.as_str()))
            .collect()
    } else {
        Vec::new()
    };

    Ok(askama_axum::IntoResponse::into_response(
        ContactGroupDetailTpl {
            me,
            group,
            members,
            candidates,
        },
    ))
}

#[derive(serde::Serialize)]
struct UpdateGroupPayload<'a> {
    name: &'a str,
    description: Option<&'a str>,
}

/// POST /contacts/groups/:id/rename — rename / re-describe a group.
async fn contact_group_rename_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    axum::Form(f): axum::Form<GroupForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let name = f.name.trim();
    if !name.is_empty() {
        let desc = f
            .description
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let _ = patch_json(
            &st,
            &st.backends.contacts,
            &format!("/api/v1/contact-groups/{enc}"),
            &headers,
            Some((&t, &u)),
            &UpdateGroupPayload {
                name,
                description: desc,
            },
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/contacts/groups/{enc}")).into_response())
}

/// POST /contacts/groups/:id/delete — delete a group.
async fn contact_group_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/contact-groups/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/contacts/groups").into_response())
}

#[derive(Deserialize)]
struct AddMemberForm {
    contact_id: String,
}

#[derive(serde::Serialize)]
struct AddMemberPayload<'a> {
    contact_id: &'a str,
}

/// POST /contacts/groups/:id/members/add — add a contact to the group.
async fn contact_group_add_member_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    axum::Form(f): axum::Form<AddMemberForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let cid = f.contact_id.trim();
    if !cid.is_empty() {
        let _ = post_json(
            &st,
            &st.backends.contacts,
            &format!("/api/v1/contact-groups/{enc}/members"),
            &headers,
            Some((&t, &u)),
            &AddMemberPayload { contact_id: cid },
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/contacts/groups/{enc}")).into_response())
}

/// POST /contacts/groups/:id/members/:cid/remove — remove a member.
async fn contact_group_remove_member_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, cid)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let enc_c = utf8_percent_encode(&cid, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/contact-groups/{enc}/members/{enc_c}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/contacts/groups/{enc}")).into_response())
}

// ─── ACL sharing pages ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ShareForm {
    email: String,
    privilege: String,
}

#[derive(serde::Serialize)]
struct ShareJsonPayload<'a> {
    grantee_id: String,
    privilege: &'a str,
}

#[derive(Deserialize)]
struct UserLookup {
    id: String,
    #[serde(default)]
    email: Option<String>,
}

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
        st,
        backend,
        &format!("/api/v1/users?email={enc}"),
        headers,
        Some((t, u)),
    )
    .await?;
    Ok(out.map(|x| {
        let _ = x.email;
        x.id
    }))
}

// ── Calendar share ──

async fn calendar_share_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let cal: Calendar = match get_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(c) => c,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let shares: Vec<AclRow> = get_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/acl"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(CalendarShareTpl {
        me,
        calendar: cal,
        shares,
        error: None,
    }
    .into_response())
}

async fn calendar_share_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    Form(f): Form<ShareForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let email = f.email.trim().to_ascii_lowercase();
    let enc_cal = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();

    let grantee_id =
        match resolve_user_id(&st, &st.backends.calendar, &email, &headers, &t, &u).await? {
            Some(id) => id,
            None => {
                return Ok(
                    Redirect::to(&format!("/calendar/{enc_cal}/share?error=user_not_found"))
                        .into_response(),
                )
            }
        };

    let payload = ShareJsonPayload {
        grantee_id,
        privilege: &f.privilege,
    };
    let status = crate::upstream::post_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_cal}/acl"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok(
            Redirect::to(&format!("/calendar/{enc_cal}/share?error=share_{status}"))
                .into_response(),
        );
    }
    Ok(Redirect::to(&format!("/calendar/{enc_cal}/share")).into_response())
}

async fn calendar_share_revoke(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cal_id, grantee_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_cal = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC).to_string();
    let enc_g = utf8_percent_encode(&grantee_id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_cal}/acl/{enc_g}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/calendar/{enc_cal}/share")).into_response())
}

async fn calendar_export_ics(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let (status, _ct, _cd, body) = get_bytes(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{}/export.ics",
            utf8_percent_encode(&cal_id, NON_ALPHANUMERIC)
        ),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    if !(200..300).contains(&(status as i32)) {
        return Ok((StatusCode::BAD_GATEWAY, "Erro ao exportar").into_response());
    }
    let cd = format!("attachment; filename=\"{cal_id}.ics\"");
    Ok((
        [
            (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
            (header::CONTENT_DISPOSITION, cd.as_str()),
        ],
        body,
    )
        .into_response())
}

async fn calendar_import_ics(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cal_id): Path<String>,
    body: axum::body::Bytes,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let url = format!(
        "{}/api/v1/calendars/{}/import",
        st.backends.calendar.trim_end_matches('/'),
        utf8_percent_encode(&cal_id, NON_ALPHANUMERIC)
    );
    let mut req = st
        .http
        .post(&url)
        .header("Content-Type", "text/calendar")
        .body(body.to_vec());
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let _ = req.send().await;
    Ok(Redirect::to(&format!(
        "/calendar/{}",
        utf8_percent_encode(&cal_id, NON_ALPHANUMERIC)
    ))
    .into_response())
}

#[derive(Deserialize)]
struct RescheduleForm {
    new_date: String,
}

async fn event_reschedule_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cal_id, event_id)): Path<(String, String)>,
    Form(f): Form<RescheduleForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "new_date": f.new_date });
    let _ = post_json(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{}/events/{}/reschedule",
            utf8_percent_encode(&cal_id, NON_ALPHANUMERIC),
            utf8_percent_encode(&event_id, NON_ALPHANUMERIC)
        ),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok((StatusCode::OK, "").into_response())
}

// ── Calendar event extend (resize) ──

#[derive(serde::Deserialize)]
struct ExtendForm {
    add_minutes: i64,
}

async fn event_extend_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cal_id, event_id)): Path<(String, String)>,
    Form(f): Form<ExtendForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "add_minutes": f.add_minutes });
    let _ = post_json(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{}/events/{}/extend",
            utf8_percent_encode(&cal_id, NON_ALPHANUMERIC),
            utf8_percent_encode(&event_id, NON_ALPHANUMERIC)
        ),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok((StatusCode::OK, "").into_response())
}

// ── Addressbook share ──

async fn addrbook_share_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(book_id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let book: AddressBook = match get_json(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    {
        Some(b) => b,
        None => return Ok(login_redirect(&uri).into_response()),
    };
    let shares: Vec<AclRow> = get_json(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc}/acl"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(AddrbookShareTpl {
        me,
        addressbook: book,
        shares,
        error: None,
    }
    .into_response())
}

async fn addrbook_share_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(book_id): Path<String>,
    Form(f): Form<ShareForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let email = f.email.trim().to_ascii_lowercase();
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();

    let grantee_id =
        match resolve_user_id(&st, &st.backends.contacts, &email, &headers, &t, &u).await? {
            Some(id) => id,
            None => {
                return Ok(
                    Redirect::to(&format!("/contacts/{enc_b}/share?error=user_not_found"))
                        .into_response(),
                )
            }
        };
    let payload = ShareJsonPayload {
        grantee_id,
        privilege: &f.privilege,
    };
    let status = crate::upstream::post_json(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/acl"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok(
            Redirect::to(&format!("/contacts/{enc_b}/share?error=share_{status}")).into_response(),
        );
    }
    Ok(Redirect::to(&format!("/contacts/{enc_b}/share")).into_response())
}

async fn addrbook_share_revoke(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((book_id, grantee_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_g = utf8_percent_encode(&grantee_id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/acl/{enc_g}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/contacts/{enc_b}/share")).into_response())
}

// ─── /chat ───────────────────────────────────────────────────────────────────

async fn chat_fetch_channels(
    st: &AppState,
    headers: &HeaderMap,
    t: &str,
    u: &str,
) -> Vec<ChatChannel> {
    let url = format!("{}/api/v1/channels", st.backends.chat.trim_end_matches('/'));
    let mut req = st.http.get(&url);
    req = crate::upstream::fwd_cookie(req, headers);
    req = crate::upstream::inject_ctx(req, t, u);
    match req.send().await {
        Ok(r) if r.status().is_success() => r.json::<Vec<ChatChannel>>().await.unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn chat_fetch_messages(
    st: &AppState,
    headers: &HeaderMap,
    t: &str,
    u: &str,
    cid: &str,
    after: Option<&str>,
) -> Vec<ChatMessage> {
    let qs = after.map(|a| format!("?after={a}")).unwrap_or_default();
    let url = format!(
        "{}/api/v1/channels/{cid}/messages{qs}",
        st.backends.chat.trim_end_matches('/')
    );
    let mut req = st.http.get(&url);
    req = crate::upstream::fwd_cookie(req, headers);
    req = crate::upstream::inject_ctx(req, t, u);
    match req.send().await {
        Ok(r) if r.status().is_success() => r.json::<Vec<ChatMessage>>().await.unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn chat_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let channels = chat_fetch_channels(&st, &headers, &t, &u).await;
    let active_channel = channels.first().cloned();
    let messages = if let Some(ref ch) = active_channel {
        chat_fetch_messages(&st, &headers, &t, &u, &ch.id, None).await
    } else {
        Vec::new()
    };
    Ok(askama_axum::IntoResponse::into_response(ChatTpl {
        me,
        channels,
        active_channel,
        messages,
    }))
}

async fn chat_channel_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let channels = chat_fetch_channels(&st, &headers, &t, &u).await;
    let active_channel = channels.iter().find(|c| c.id == cid).cloned();
    let messages = chat_fetch_messages(&st, &headers, &t, &u, &cid, None).await;
    Ok(askama_axum::IntoResponse::into_response(ChatTpl {
        me,
        channels,
        active_channel,
        messages,
    }))
}

#[derive(Deserialize)]
struct ChatCreateChannelForm {
    name: String,
    kind: Option<String>,
}

async fn chat_create_channel(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<ChatCreateChannelForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({
        "name": f.name.trim(),
        "kind": f.kind.as_deref().unwrap_or("public"),
    });
    let url = format!("{}/api/v1/channels", st.backends.chat.trim_end_matches('/'));
    let mut req = st.http.post(&url).json(&payload);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let created: Option<ChatChannel> = match req.send().await {
        Ok(r) if r.status().is_success() => r.json().await.ok(),
        _ => None,
    };
    if let Some(ch) = created {
        Ok(Redirect::to(&format!("/chat/channels/{}", ch.id)).into_response())
    } else {
        Ok(Redirect::to("/chat").into_response())
    }
}

#[derive(Deserialize)]
struct ChatSendForm {
    body: String,
    #[serde(default)]
    parent_id: Option<String>,
}

async fn chat_send_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
    Form(f): Form<ChatSendForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let mut payload = serde_json::json!({ "body": f.body.trim() });
    if let Some(pid) = &f.parent_id {
        payload["parent_id"] = serde_json::json!(pid);
    }
    let url = format!(
        "{}/api/v1/channels/{cid}/messages",
        st.backends.chat.trim_end_matches('/')
    );
    let mut req = st.http.post(&url).json(&payload);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let _ = req.send().await;
    Ok(Redirect::to(&format!("/chat/channels/{cid}")).into_response())
}

#[derive(serde::Serialize)]
struct ChatPollResp {
    messages: Vec<ChatMessage>,
}

async fn chat_poll_messages(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let after = q.get("after").map(String::as_str);
    let messages = chat_fetch_messages(&st, &headers, &t, &u, &cid, after).await;
    Ok(axum::Json(ChatPollResp { messages }).into_response())
}

#[derive(Deserialize)]
struct ChatReactForm {
    emoji: String,
}

async fn chat_mark_read(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{cid}/mark-read"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok((StatusCode::OK, "ok").into_response())
}

#[derive(serde::Serialize)]
struct ChatReactResp {
    reactions: std::collections::HashMap<String, u32>,
}

async fn chat_react_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cid, mid)): Path<(String, String)>,
    Form(f): Form<ChatReactForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "emoji": f.emoji.trim() });
    let url = format!(
        "{}/api/v1/channels/{cid}/messages/{mid}/reactions",
        st.backends.chat.trim_end_matches('/')
    );
    let mut req = st.http.post(&url).json(&payload);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let reactions: std::collections::HashMap<String, u32> = match req.send().await {
        Ok(r) if r.status().is_success() => r.json().await.unwrap_or_default(),
        _ => std::collections::HashMap::new(),
    };
    Ok(axum::Json(ChatReactResp { reactions }).into_response())
}

// ── Chat pin ──

#[derive(serde::Deserialize)]
struct ChatPinForm {
    message_id: String,
}

async fn chat_get_pin(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let data: serde_json::Value = get_json(
        &st,
        &st.backends.chat,
        &format!(
            "/api/v1/channels/{}/pin",
            utf8_percent_encode(&cid, NON_ALPHANUMERIC)
        ),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::Value::Null);
    Ok(axum::Json(data).into_response())
}

async fn chat_set_pin(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
    Form(f): Form<ChatPinForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({ "message_id": f.message_id });
    let _ = post_json(
        &st,
        &st.backends.chat,
        &format!(
            "/api/v1/channels/{}/pin",
            utf8_percent_encode(&cid, NON_ALPHANUMERIC)
        ),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok((StatusCode::OK, "").into_response())
}

async fn chat_delete_pin(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let url = format!(
        "{}/api/v1/channels/{}/pin",
        st.backends.chat.trim_end_matches('/'),
        utf8_percent_encode(&cid, NON_ALPHANUMERIC)
    );
    let mut req = st.http.delete(&url);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let _ = req.send().await;
    Ok((StatusCode::OK, "").into_response())
}

// ─── /meet ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MeetPageQuery {
    flash: Option<String>,
}

async fn meet_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<MeetPageQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let meetings: Vec<MeetRoom> = get_json(
        &st,
        &st.backends.meet,
        "/api/v1/meetings",
        &headers,
        Some((&me.tenant_id, &me.user_id)),
    )
    .await?
    .unwrap_or_default();
    let now_iso = chrono_now_iso();
    let mut upcoming: Vec<MeetRoom> = meetings
        .iter()
        .filter(|m| {
            !m.is_ended()
                && m.scheduled_at
                    .as_deref()
                    .map(|s| s >= now_iso.as_str())
                    .unwrap_or(true)
        })
        .cloned()
        .collect();
    let mut past: Vec<MeetRoom> = meetings
        .iter()
        .filter(|m| {
            m.is_ended()
                || m.scheduled_at
                    .as_deref()
                    .map(|s| s < now_iso.as_str())
                    .unwrap_or(false)
        })
        .cloned()
        .collect();
    upcoming.sort_by(|a, b| a.scheduled_at.cmp(&b.scheduled_at));
    past.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(askama_axum::IntoResponse::into_response(MeetTpl {
        me,
        meetings,
        upcoming,
        past,
        flash: q.flash,
    }))
}

fn chrono_now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d, h, mi) = secs_to_ymdhm(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:00+00:00")
}

fn secs_to_ymdhm(mut s: u64) -> (u32, u32, u32, u32, u32) {
    let mi = (s % 60) as u32;
    s /= 60;
    let h = (s % 24) as u32;
    s /= 24;
    // Simplified date from epoch (good until 2100)
    let mut y = 1970u32;
    loop {
        let days_in_year =
            if y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400)) {
                366
            } else {
                365
            };
        if s < days_in_year {
            break;
        }
        s -= days_in_year;
        y += 1;
    }
    let leap = y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400));
    let months = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u32;
    for &dm in &months {
        if s < dm {
            break;
        }
        s -= dm;
        mo += 1;
    }
    (y, mo, s as u32 + 1, h, mi)
}

async fn meet_new_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(MeetRoomTpl {
        me,
        room_id: String::new(),
        room_name: String::new(),
        meeting: None,
        participants: Vec::new(),
        jitsi_domain: st.jitsi.domain.clone(),
        jitsi_jwt: String::new(),
        jitsi_enabled: st.jitsi.is_enabled(),
        join_only: false,
        is_moderator: true,
    }))
}

#[derive(Deserialize)]
struct MeetCreateForm {
    name: Option<String>,
}

#[derive(serde::Deserialize)]
#[allow(dead_code)] // `name` is part of the API payload but not consumed here
struct MeetCreated {
    id: String,
    #[serde(default)]
    name: Option<String>,
}

#[derive(serde::Deserialize)]
struct MeetTokenResp {
    token: String,
}

async fn meet_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<MeetCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let name = f.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        format!(
            "Reunião de {}",
            me.display_name.as_deref().unwrap_or(&me.email)
        )
    });
    let payload = serde_json::json!({ "name": name });
    let meeting: Option<MeetCreated> = {
        let url = format!("{}/api/v1/meetings", st.backends.meet.trim_end_matches('/'));
        let mut req = st.http.post(&url).json(&payload);
        req = crate::upstream::fwd_cookie(req, &headers);
        req = crate::upstream::inject_ctx(req, &t, &u);
        let resp = req.send().await?;
        if resp.status().is_success() {
            resp.json().await.ok()
        } else {
            None
        }
    };
    match meeting {
        Some(m) => Ok(Redirect::to(&format!("/meet/{}", m.id)).into_response()),
        None => Ok(Redirect::to("/meet").into_response()),
    }
}

async fn meet_schedule_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(MeetScheduleTpl {
        me,
        error: None,
    }))
}

#[derive(Deserialize)]
struct MeetScheduleForm {
    name: String,
    scheduled_at: String, // datetime-local "YYYY-MM-DDTHH:MM"
    #[serde(default)]
    scheduled_end: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

async fn meet_schedule_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<MeetScheduleForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if f.name.trim().is_empty() || f.scheduled_at.trim().is_empty() {
        return Ok(askama_axum::IntoResponse::into_response(MeetScheduleTpl {
            me,
            error: Some("Nome e horário são obrigatórios.".into()),
        }));
    }
    let (t, u) = ctx_of(&me);
    let payload = serde_json::json!({
        "name": f.name.trim(),
        "scheduled_at": format!("{}:00+00:00", f.scheduled_at.trim()),
        "scheduled_end": f.scheduled_end.filter(|s| !s.trim().is_empty())
            .map(|s| format!("{}:00+00:00", s.trim())),
        "description": f.description.filter(|s| !s.trim().is_empty()),
    });
    let url = format!("{}/api/v1/meetings", st.backends.meet.trim_end_matches('/'));
    let mut req = st.http.post(&url).json(&payload);
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let ok = req
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    if ok {
        Ok(Redirect::to("/meet?flash=Reunião+agendada+com+sucesso").into_response())
    } else {
        Ok(askama_axum::IntoResponse::into_response(MeetScheduleTpl {
            me,
            error: Some("Falha ao agendar. Tente novamente.".into()),
        }))
    }
}

#[derive(Deserialize)]
struct MeetJoinQuery {
    room: Option<String>,
}

async fn meet_join_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<MeetJoinQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let room_name = q
        .room
        .unwrap_or_else(|| format!("{}{}", st.jitsi.room_prefix, uuid_v4()));
    Ok(askama_axum::IntoResponse::into_response(MeetRoomTpl {
        me,
        room_id: room_name.clone(),
        room_name,
        meeting: None,
        participants: Vec::new(),
        jitsi_domain: st.jitsi.domain.clone(),
        jitsi_jwt: String::new(),
        jitsi_enabled: st.jitsi.is_enabled(),
        join_only: true,
        is_moderator: false,
    }))
}

async fn meet_room_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let meeting: Option<MeetRoom> = {
        let url = format!(
            "{}/api/v1/meetings/{}",
            st.backends.meet.trim_end_matches('/'),
            id
        );
        let mut req = st.http.get(&url);
        req = crate::upstream::fwd_cookie(req, &headers);
        req = crate::upstream::inject_ctx(req, &t, &u);
        match req.send().await {
            Ok(r) if r.status().is_success() => r.json::<MeetRoom>().await.ok(),
            _ => None,
        }
    };
    let participants: Vec<MeetParticipant> = {
        let url = format!(
            "{}/api/v1/meetings/{}/participants",
            st.backends.meet.trim_end_matches('/'),
            id
        );
        let mut req = st.http.get(&url);
        req = crate::upstream::fwd_cookie(req, &headers);
        req = crate::upstream::inject_ctx(req, &t, &u);
        match req.send().await {
            Ok(r) if r.status().is_success() => {
                r.json::<Vec<MeetParticipant>>().await.unwrap_or_default()
            }
            _ => Vec::new(),
        }
    };
    let token_resp: Option<MeetTokenResp> = {
        let url = format!(
            "{}/api/v1/meetings/{}/tokens",
            st.backends.meet.trim_end_matches('/'),
            id
        );
        let mut req = st
            .http
            .post(&url)
            .json(&serde_json::json!({"role":"participant"}));
        req = crate::upstream::fwd_cookie(req, &headers);
        req = crate::upstream::inject_ctx(req, &t, &u);
        let resp = req.send().await?;
        if resp.status().is_success() {
            resp.json().await.ok()
        } else {
            None
        }
    };
    let jwt = token_resp.map(|r| r.token).unwrap_or_default();
    let room_name = format!("{}{}", st.jitsi.room_prefix, id);
    Ok(askama_axum::IntoResponse::into_response(MeetRoomTpl {
        me,
        room_id: id,
        room_name,
        meeting,
        participants,
        jitsi_domain: st.jitsi.domain.clone(),
        jitsi_jwt: jwt,
        jitsi_enabled: st.jitsi.is_enabled(),
        join_only: false,
        is_moderator: false,
    }))
}

async fn meet_end_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let url = format!(
        "{}/api/v1/meetings/{}/end",
        st.backends.meet.trim_end_matches('/'),
        id
    );
    let mut req = st.http.post(&url).json(&serde_json::json!({}));
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let _ = req.send().await;
    Ok(Redirect::to("/meet").into_response())
}

async fn meet_recordings_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let recs = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/recordings"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!([]));
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&recs).unwrap_or_default(),
    )
        .into_response())
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:032x}", t)
}

// ─── /tasks ──────────────────────────────────────────────────────────────────

async fn tasks_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(askama_axum::IntoResponse::into_response(TasksTpl { me }))
}

// ─── /notes ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct NotesQuery {
    id: Option<String>,
}

#[derive(Deserialize)]
struct NoteForm {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
}

/// GET /notes[?id=] — list the caller's notes; when `id` is given, open it in the
/// editor pane. Backed by the expresso-notes service.
async fn notes_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<NotesQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let notes = get_json::<Vec<Note>>(
        &st,
        &st.backends.notes,
        "/api/v1/notes",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let selected =
        q.id.as_ref()
            .and_then(|id| notes.iter().find(|n| &n.id == id))
            .map(|n| Note {
                id: n.id.clone(),
                title: n.title.clone(),
                body: n.body.clone(),
                color: n.color.clone(),
                pinned: n.pinned,
            });
    Ok(askama_axum::IntoResponse::into_response(NotesTpl {
        me,
        notes,
        selected,
    }))
}

/// POST /notes — create a note, then redirect to it.
async fn notes_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<NoteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let body = serde_json::json!({ "title": f.title, "body": f.body });
    let status = post_json(
        &st,
        &st.backends.notes,
        "/api/v1/notes",
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    Ok(Redirect::to("/notes").into_response())
}

/// POST /notes/:id — update a note's title/body, then reopen it.
async fn notes_edit_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<NoteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let body = serde_json::json!({ "title": f.title, "body": f.body });
    let status = patch_json(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, format!("upstream {status}")).into_response());
    }
    Ok(Redirect::to(&format!("/notes?id={enc}")).into_response())
}

/// POST /notes/:id/delete — delete a note, then back to the list.
async fn notes_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/notes").into_response())
}

// ─── /settings ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SettingsQuery {
    tab: Option<String>,
    flash: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)] // form fields accepted from the wire; not all read in this handler
struct ProfileForm {
    display_name: Option<String>,
    locale: Option<String>,
}

#[derive(Deserialize)]
struct SignatureForm {
    enabled: Option<String>,
    body: Option<String>,
}

#[derive(Deserialize)]
struct AutoreplyForm {
    enabled: Option<String>,
    subject: Option<String>,
    body: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)] // notification-pref form fields accepted from the wire
struct NotificationsForm {
    notify_new_mail: Option<String>,
    notify_calendar: Option<String>,
    notify_shared: Option<String>,
    browser_push: Option<String>,
}

#[derive(Deserialize)]
struct FiltersForm {
    script: Option<String>,
}

async fn settings_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<SettingsQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tab = q.tab.unwrap_or_else(|| "profile".into());

    // Load sieve script only when on filters tab
    let (sieve_script, sieve_error) = if tab == "filters" {
        match get_json::<serde_json::Value>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/sieve",
            &headers,
            Some((&t, &u)),
        )
        .await
        {
            Ok(Some(v)) => (
                v.get("script").and_then(|s| s.as_str()).map(String::from),
                None,
            ),
            Ok(None) => (None, None),
            Err(e) => (None, Some(format!("{e}"))),
        }
    } else {
        (None, None)
    };

    // Load vacation (autoreply) settings
    let vacation = if tab == "autoreply" {
        get_json::<serde_json::Value>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/vacation",
            &headers,
            Some((&t, &u)),
        )
        .await
        .ok()
        .flatten()
    } else {
        None
    };

    let autoreply_enabled = vacation
        .as_ref()
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let autoreply_subject = vacation
        .as_ref()
        .and_then(|v| v.get("subject"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let autoreply_body = vacation
        .as_ref()
        .and_then(|v| v.get("body"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let autoreply_start = vacation
        .as_ref()
        .and_then(|v| v.get("start_date"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let autoreply_end = vacation
        .as_ref()
        .and_then(|v| v.get("end_date"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Load signature settings from mail service
    let sig_data = if tab == "signature" {
        get_json::<serde_json::Value>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/signature",
            &headers,
            Some((&t, &u)),
        )
        .await
        .ok()
        .flatten()
    } else {
        None
    };
    let signature_enabled = sig_data
        .as_ref()
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let signature_body = sig_data
        .as_ref()
        .and_then(|v| v.get("body"))
        .and_then(|v| v.as_str())
        .map(String::from);

    // Working-hours editor rows, loaded only on that tab.
    let working_days = if tab == "working_hours" {
        let hours = get_json::<Vec<WorkingHour>>(
            &st,
            &st.backends.calendar,
            "/api/v1/working-hours",
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        build_working_days(&hours)
    } else {
        build_working_days(&[])
    };

    // Load tenant email aliases only on the aliases tab.
    let aliases = if tab == "aliases" {
        get_json::<Vec<MailAlias>>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/aliases",
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    Ok(askama_axum::IntoResponse::into_response(SettingsTpl {
        tab,
        flash: q.flash,
        logout_url: st.public.auth_logout_path.clone(),
        kc_account: st.public.kc_account.clone(),
        signature_enabled,
        signature_body,
        autoreply_enabled,
        autoreply_subject,
        autoreply_body,
        autoreply_start,
        autoreply_end,
        sieve_script,
        sieve_error,
        aliases,
        working_days,
        me,
    }))
}

const WEEKDAY_LABELS: [&str; 7] = [
    "Segunda", "Terça", "Quarta", "Quinta", "Sexta", "Sábado", "Domingo",
];

/// Minutes-from-midnight → "HH:MM".
fn min_to_hhmm(m: i32) -> String {
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// "HH:MM" → minutes-from-midnight, or None if malformed/out of range.
fn hhmm_to_min(s: &str) -> Option<i32> {
    let (h, m) = s.trim().split_once(':')?;
    let h: i32 = h.parse().ok()?;
    let m: i32 = m.parse().ok()?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    Some(h * 60 + m)
}

/// Build a 7-row Mon..Sun table from the backend windows (one window per day).
fn build_working_days(hours: &[WorkingHour]) -> Vec<crate::templates::WorkingDayRow> {
    (0..7i16)
        .map(|wd| {
            let win = hours.iter().find(|h| h.weekday == wd);
            crate::templates::WorkingDayRow {
                weekday: wd,
                label: WEEKDAY_LABELS[wd as usize].to_string(),
                enabled: win.is_some(),
                start: win.map_or_else(|| "09:00".into(), |h| min_to_hhmm(h.start_minute)),
                end: win.map_or_else(|| "18:00".into(), |h| min_to_hhmm(h.end_minute)),
            }
        })
        .collect()
}

async fn settings_profile_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(_f): Form<ProfileForm>,
) -> WebResult<Response> {
    let Some(_me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    Ok(Redirect::to("/settings?tab=profile&flash=Perfil+atualizado").into_response())
}

async fn settings_signature_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<SignatureForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let script_body = f.body.unwrap_or_default();
    let enabled = f.enabled.as_deref() == Some("1");
    let payload = serde_json::json!({
        "signature": { "enabled": enabled, "body": script_body }
    });
    let _ = put_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/settings",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok(Redirect::to("/settings?tab=signature&flash=Assinatura+salva").into_response())
}

async fn settings_autoreply_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<AutoreplyForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enabled = f.enabled.as_deref() == Some("1");
    let payload = serde_json::json!({
        "enabled":    enabled,
        "subject":    f.subject.unwrap_or_default(),
        "body":       f.body.unwrap_or_default(),
        "start_date": f.start_date,
        "end_date":   f.end_date,
    });
    let _ = put_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/vacation",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok(Redirect::to("/settings?tab=autoreply&flash=Resposta+automática+salva").into_response())
}

async fn settings_notifications_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(_f): Form<NotificationsForm>,
) -> WebResult<Response> {
    let Some(_me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    // Notification preferences are stored client-side (no dedicated backend endpoint)
    Ok(Redirect::to("/settings?tab=notifications&flash=Preferências+salvas").into_response())
}

async fn settings_filters_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FiltersForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let script = f.script.unwrap_or_default();
    let payload = serde_json::json!({ "script": script, "enabled": true });
    let _ = put_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/sieve",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok(Redirect::to("/settings?tab=filters&flash=Filtros+salvos").into_response())
}

#[derive(serde::Serialize)]
struct WorkingHourOut {
    weekday: i16,
    start_minute: i32,
    end_minute: i32,
}

/// POST /settings/working-hours — replace the caller's weekly working hours.
/// The form carries `on_<wd>` (checkbox), `start_<wd>`, `end_<wd>` for wd 0..6;
/// only enabled days with a valid start<end window are sent.
async fn settings_working_hours_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(fields): Form<std::collections::HashMap<String, String>>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let mut hours = Vec::new();
    for wd in 0..7i16 {
        if !fields.contains_key(&format!("on_{wd}")) {
            continue;
        }
        let start = fields
            .get(&format!("start_{wd}"))
            .and_then(|s| hhmm_to_min(s));
        let end = fields
            .get(&format!("end_{wd}"))
            .and_then(|s| hhmm_to_min(s));
        if let (Some(start_minute), Some(end_minute)) = (start, end) {
            if end_minute > start_minute {
                hours.push(WorkingHourOut {
                    weekday: wd,
                    start_minute,
                    end_minute,
                });
            }
        }
    }
    let _ = put_json(
        &st,
        &st.backends.calendar,
        "/api/v1/working-hours",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "hours": hours }),
    )
    .await?;
    Ok(Redirect::to("/settings?tab=working_hours&flash=Horários+salvos").into_response())
}

#[derive(Deserialize)]
struct AliasForm {
    alias: String,
    target: String,
}

#[derive(serde::Serialize)]
struct NewAliasPayload<'a> {
    alias: &'a str,
    target: &'a str,
}

/// POST /settings/aliases — create a tenant email alias.
async fn settings_alias_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<AliasForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let alias = f.alias.trim();
    let target = f.target.trim();
    if alias.is_empty() || target.is_empty() {
        return Ok(
            Redirect::to("/settings?tab=aliases&flash=Preencha+alias+e+destino").into_response(),
        );
    }
    let status = post_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/aliases",
        &headers,
        Some((&t, &u)),
        &NewAliasPayload { alias, target },
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Alias+criado"
    } else {
        "Alias+inválido+ou+já+existe"
    };
    Ok(Redirect::to(&format!("/settings?tab=aliases&flash={flash}")).into_response())
}

#[derive(Deserialize)]
struct AliasToggleForm {
    target: String,
    /// "1" when the box is checked → enable; absent → disable.
    #[serde(default)]
    is_enabled: Option<String>,
}

#[derive(serde::Serialize)]
struct UpdateAliasPayload<'a> {
    target: &'a str,
    is_enabled: bool,
}

/// POST /settings/aliases/:id/toggle — enable/disable an alias (PUT upstream).
async fn settings_alias_toggle(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<AliasToggleForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = put_json(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/aliases/{enc}"),
        &headers,
        Some((&t, &u)),
        &UpdateAliasPayload {
            target: f.target.trim(),
            is_enabled: f.is_enabled.is_some(),
        },
    )
    .await?;
    Ok(Redirect::to("/settings?tab=aliases&flash=Alias+atualizado").into_response())
}

/// POST /settings/aliases/:id/delete — remove an alias.
async fn settings_alias_delete(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/aliases/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/settings?tab=aliases&flash=Alias+removido").into_response())
}

// ─── mailbox delegation ──────────────────────────────────────────────────────

/// Resolve a tenant user id to its email via the contacts user-lookup; falls
/// back to the id string when the lookup fails (best-effort display).
async fn resolve_email_by_id(
    st: &AppState,
    id: &str,
    headers: &HeaderMap,
    t: &str,
    u: &str,
) -> String {
    let enc = utf8_percent_encode(id, NON_ALPHANUMERIC).to_string();
    match get_json::<UserLookup>(
        st,
        &st.backends.contacts,
        &format!("/api/v1/users?id={enc}"),
        headers,
        Some((t, u)),
    )
    .await
    {
        Ok(Some(x)) => x.email.unwrap_or_else(|| id.to_string()),
        _ => id.to_string(),
    }
}

/// Turn raw delegation rows into display rows, resolving each counterparty id
/// (selected by `show_owner`: owner for given-to-me, delegate for given-by-me)
/// to an email.
async fn delegation_views(
    st: &AppState,
    raw: Vec<DelegationRaw>,
    show_owner: bool,
    headers: &HeaderMap,
    t: &str,
    u: &str,
) -> Vec<DelegationView> {
    let mut out = Vec::with_capacity(raw.len());
    for d in raw {
        let who_id = if show_owner {
            d.owner_id
        } else {
            d.delegate_id
        };
        let who = resolve_email_by_id(st, &who_id, headers, t, u).await;
        out.push(DelegationView {
            id: d.id,
            who,
            access: d.access,
            who_id,
        });
    }
    out
}

#[derive(Deserialize)]
struct DelegationQuery {
    flash: Option<String>,
}

/// GET /settings/delegations — mailbox delegation management screen.
async fn delegations_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<DelegationQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let granted_raw = get_json::<Vec<DelegationRaw>>(
        &st,
        &st.backends.mail,
        "/api/v1/mail/delegations",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let to_me_raw = get_json::<Vec<DelegationRaw>>(
        &st,
        &st.backends.mail,
        "/api/v1/mail/delegations/to-me",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let granted = delegation_views(&st, granted_raw, false, &headers, &t, &u).await;
    let to_me = delegation_views(&st, to_me_raw, true, &headers, &t, &u).await;

    Ok(askama_axum::IntoResponse::into_response(DelegationsTpl {
        me,
        flash: q.flash,
        granted,
        to_me,
    }))
}

#[derive(Deserialize)]
struct DelegationGrantForm {
    email: String,
    access: String,
}

#[derive(serde::Serialize)]
struct GrantPayload<'a> {
    delegate_id: &'a str,
    access: &'a str,
}

/// POST /settings/delegations — grant a tenant user access to the caller's
/// mailbox. The delegate is identified by email, resolved to a user id.
async fn delegation_grant_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<DelegationGrantForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let email = f.email.trim();
    let access = match f.access.trim().to_ascii_uppercase().as_str() {
        "READ" => "READ",
        "SEND" => "SEND",
        _ => return Ok(Redirect::to("/settings/delegations?flash=Acesso+inválido").into_response()),
    };
    let Some(delegate_id) =
        resolve_user_id(&st, &st.backends.contacts, email, &headers, &t, &u).await?
    else {
        return Ok(
            Redirect::to("/settings/delegations?flash=Usuário+não+encontrado").into_response(),
        );
    };
    let status = post_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/delegations",
        &headers,
        Some((&t, &u)),
        &GrantPayload {
            delegate_id: &delegate_id,
            access,
        },
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Delegação+criada"
    } else {
        "Não+foi+possível+delegar"
    };
    Ok(Redirect::to(&format!("/settings/delegations?flash={flash}")).into_response())
}

/// POST /settings/delegations/:id/revoke — revoke a grant the caller owns.
async fn delegation_revoke_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/delegations/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/settings/delegations?flash=Delegação+revogada").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::Me;

    /// Build a minimal `Me` for ctx_of/tenant tests — only tenant_id/user_id
    /// matter to those; the rest are filled with empty defaults.
    fn mk_me(tenant_id: &str, user_id: &str) -> Me {
        Me {
            user_id: user_id.into(),
            tenant_id: tenant_id.into(),
            email: String::new(),
            display_name: None,
            roles: Vec::new(),
            expires_at: 0,
            mfa: None,
        }
    }

    #[test]
    fn split_addrs_comma_separated() {
        let v = split_addrs("a@ex.com,b@ex.com");
        assert_eq!(v, vec!["a@ex.com", "b@ex.com"]);
    }

    #[test]
    fn bulk_payload_splits_ids_and_nulls_blank_parent() {
        let v = bulk_payload(" a , b ,, c ", "  ");
        assert_eq!(v["ids"], serde_json::json!(["a", "b", "c"]));
        assert!(v["parent_id"].is_null());
    }

    #[test]
    fn bulk_payload_keeps_parent_when_set() {
        let v = bulk_payload("x", "folder-1");
        assert_eq!(v["parent_id"], "folder-1");
    }

    #[test]
    fn hhmm_min_roundtrip() {
        assert_eq!(hhmm_to_min("09:00"), Some(540));
        assert_eq!(hhmm_to_min("18:30"), Some(1110));
        assert_eq!(min_to_hhmm(540), "09:00");
        assert_eq!(min_to_hhmm(1110), "18:30");
        assert_eq!(hhmm_to_min("24:00"), None);
        assert_eq!(hhmm_to_min("9:60"), None);
        assert_eq!(hhmm_to_min("garbage"), None);
    }

    #[test]
    fn to_rfc3339_adds_seconds_and_utc() {
        assert_eq!(
            to_rfc3339("2026-06-10T14:30"),
            Some("2026-06-10T14:30:00Z".into())
        );
    }

    #[test]
    fn to_rfc3339_passes_through_offset_and_z() {
        assert_eq!(
            to_rfc3339("2026-06-10T14:30:00Z"),
            Some("2026-06-10T14:30:00Z".into())
        );
        assert_eq!(
            to_rfc3339("2026-06-10T14:30:00-03:00"),
            Some("2026-06-10T14:30:00-03:00".into())
        );
    }

    #[test]
    fn to_rfc3339_blank_is_none() {
        assert_eq!(to_rfc3339(""), None);
        assert_eq!(to_rfc3339("   "), None);
    }

    #[test]
    fn with_obo_appends_param_with_correct_separator() {
        assert_eq!(
            with_obo("/api/v1/mail/folders".into(), Some("abc-123")),
            "/api/v1/mail/folders?on_behalf_of=abc%2D123"
        );
        assert_eq!(
            with_obo("/api/v1/mail/messages?folder=INBOX".into(), Some("u1")),
            "/api/v1/mail/messages?folder=INBOX&on_behalf_of=u1"
        );
    }

    #[test]
    fn with_obo_noop_when_blank_or_none() {
        assert_eq!(with_obo("/x".into(), None), "/x");
        assert_eq!(with_obo("/x".into(), Some("")), "/x");
        assert_eq!(with_obo("/x".into(), Some("  ")), "/x");
    }

    #[test]
    fn build_valarms_emits_one_block_per_minute() {
        let out = build_valarms("15,60", "Reunião");
        assert_eq!(out.matches("BEGIN:VALARM").count(), 2);
        assert!(out.contains("TRIGGER:-PT15M"));
        assert!(out.contains("TRIGGER:-PT60M"));
        assert!(out.contains("ACTION:DISPLAY"));
    }

    #[test]
    fn build_valarms_dedups_sorts_and_skips_junk() {
        let out = build_valarms("60, 15, 15, x, -5", "x");
        // 15 and 60 survive (dedup); junk and negative dropped.
        assert_eq!(out.matches("BEGIN:VALARM").count(), 2);
        let p15 = out.find("PT15M").unwrap();
        let p60 = out.find("PT60M").unwrap();
        assert!(p15 < p60, "sorted ascending");
    }

    #[test]
    fn build_valarms_empty_is_empty() {
        assert_eq!(build_valarms("", "x"), "");
        assert_eq!(build_valarms("  ,  ", "x"), "");
    }

    #[test]
    fn valarm_minutes_extracts_from_ical() {
        let ical = "BEGIN:VALARM\r\nTRIGGER:-PT15M\r\nEND:VALARM\r\n\
                    BEGIN:VALARM\r\nTRIGGER:-PT1H\r\nEND:VALARM\r\n";
        assert_eq!(valarm_minutes(ical), "15,60");
    }

    #[test]
    fn valarm_minutes_handles_days_and_dedup() {
        let ical = "TRIGGER:-P1D\r\nTRIGGER:-PT60M\r\nTRIGGER:-PT60M\r\n";
        assert_eq!(valarm_minutes(ical), "60,1440");
    }

    #[test]
    fn valarm_minutes_empty_when_none() {
        assert_eq!(valarm_minutes("BEGIN:VEVENT\r\nEND:VEVENT\r\n"), "");
    }

    #[test]
    fn parse_neg_duration_minutes_variants() {
        assert_eq!(parse_neg_duration_minutes("T15M"), Some(15));
        assert_eq!(parse_neg_duration_minutes("T2H"), Some(120));
        assert_eq!(parse_neg_duration_minutes("1D"), Some(1440));
        assert_eq!(parse_neg_duration_minutes("T1H30M"), None); // compound unsupported
        assert_eq!(parse_neg_duration_minutes("xyz"), None);
    }

    #[test]
    fn split_addrs_semicolon_separated() {
        let v = split_addrs("a@ex.com; b@ex.com");
        assert_eq!(v, vec!["a@ex.com", "b@ex.com"]);
    }

    #[test]
    fn split_addrs_trims_whitespace() {
        let v = split_addrs("  a@ex.com ,  b@ex.com  ");
        assert_eq!(v, vec!["a@ex.com", "b@ex.com"]);
    }

    #[test]
    fn split_addrs_filters_empty_segments() {
        let v = split_addrs(",, a@ex.com ,,");
        assert_eq!(v, vec!["a@ex.com"]);
    }

    #[test]
    fn split_addrs_empty_string_returns_empty() {
        assert!(split_addrs("").is_empty());
    }

    #[test]
    fn split_addrs_single_addr() {
        let v = split_addrs("user@mail.example");
        assert_eq!(v, vec!["user@mail.example"]);
    }

    #[test]
    fn ctx_of_returns_tenant_and_user() {
        let me = mk_me("t1", "u1");
        let (t, u) = ctx_of(&me);
        assert_eq!(t, "t1");
        assert_eq!(u, "u1");
    }

    #[test]
    fn ctx_of_tenant_id_matches_me() {
        let me = mk_me("acme", "uuid-abc");
        let (t, _) = ctx_of(&me);
        assert_eq!(t, "acme");
    }

    #[test]
    fn ctx_of_user_id_matches_me() {
        let me = mk_me("t2", "user-xyz");
        let (_, u) = ctx_of(&me);
        assert_eq!(u, "user-xyz");
    }

    #[test]
    fn ctx_of_returns_two_elements() {
        let me = mk_me("t3", "u3");
        let (t, u) = ctx_of(&me);
        assert!(!t.is_empty());
        assert!(!u.is_empty());
    }

    #[test]
    fn ctx_of_values_match_me_fields() {
        let me = mk_me("tenant-xyz", "user-abc");
        let (t, u) = ctx_of(&me);
        assert_eq!(t, "tenant-xyz");
        assert_eq!(u, "user-abc");
    }

    #[test]
    fn ctx_of_empty_ids_returns_empty_strings() {
        let me = mk_me("", "");
        let (t, u) = ctx_of(&me);
        assert!(t.is_empty() && u.is_empty());
    }

    #[test]
    fn ctx_of_special_chars_preserved() {
        let me = mk_me("tenant-with-dashes", "user_with_underscores");
        let (t, u) = ctx_of(&me);
        assert!(t.contains('-'));
        assert!(u.contains('_'));
    }

    #[test]
    fn ctx_of_non_empty_tenant_and_user() {
        let me = mk_me("acme-corp", "user-42");
        let (t, u) = ctx_of(&me);
        assert!(!t.is_empty());
        assert!(!u.is_empty());
    }

    #[test]
    fn ctx_of_uuid_format_preserved() {
        let me = mk_me(
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
        );
        let (t, u) = ctx_of(&me);
        assert_eq!(t, "00000000-0000-0000-0000-000000000001");
        assert_eq!(u, "00000000-0000-0000-0000-000000000002");
    }

    #[test]
    fn split_addrs_multiple_addresses() {
        let v = split_addrs("a@x.com, b@y.com, c@z.com");
        assert_eq!(v.len(), 3);
        assert!(v.iter().any(|s| s == "a@x.com"));
    }

    #[test]
    fn split_addrs_single_address_returns_one_element() {
        let v = split_addrs("solo@example.com");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], "solo@example.com");
    }

    #[test]
    fn ctx_of_tenant_and_user_are_independent() {
        let me = mk_me("t1", "u1");
        let (t, u) = ctx_of(&me);
        assert_ne!(t, u);
    }

    #[test]
    fn split_addrs_preserves_single_addr_unchanged() {
        let v = split_addrs("alice@example.com");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0], "alice@example.com");
    }

    #[test]
    fn split_addrs_multiple_commas_yield_correct_count() {
        let v = split_addrs("a@x.com,b@x.com,c@x.com");
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn split_addrs_empty_string_returns_empty_vec() {
        let v = split_addrs("");
        assert!(v.is_empty());
    }

    #[test]
    fn split_addrs_whitespace_only_returns_empty() {
        let v = split_addrs("   ");
        assert!(v.is_empty());
    }

    #[test]
    fn split_addrs_tab_separated_returns_empty_after_filter() {
        let v = split_addrs("\t");
        assert!(v.is_empty());
    }
}

// ─── Admin helpers ────────────────────────────────────────────────────────────

fn require_admin(me: &Me) -> bool {
    me.roles.iter().any(|r| r == "admin" || r == "superadmin")
}

async fn admin_redirect() -> impl IntoResponse {
    Redirect::to("/admin/users")
}

// ─── /admin/users ─────────────────────────────────────────────────────────────

async fn admin_users_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let users = get_json::<Vec<AdminUser>>(
        &st,
        &st.backends.auth,
        "/api/v1/admin/users",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let flash = extract_flash(&uri);
    Ok(askama_axum::IntoResponse::into_response(AdminUsersTpl {
        me,
        users,
        flash,
    }))
}

async fn admin_user_detail_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let user = get_json::<AdminUser>(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| AdminUser {
        id: id.clone(),
        email: id.clone(),
        display_name: None,
        role: String::new(),
        status: String::new(),
        tenant_id: String::new(),
        created_at: None,
    });
    let logins = get_json::<Vec<AdminLoginEvent>>(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/logins"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let flash = extract_flash(&uri);
    Ok(askama_axum::IntoResponse::into_response(
        AdminUserDetailTpl {
            me,
            user,
            logins,
            flash,
        },
    ))
}

#[derive(Deserialize)]
struct AdminUserQuotaForm {
    #[serde(default)]
    mail_quota_mb: Option<i64>,
    #[serde(default)]
    drive_quota_gb: Option<i64>,
    #[serde(default)]
    max_recipients_day: Option<i64>,
    #[serde(default)]
    max_attach_mb: Option<i64>,
}
async fn admin_user_set_quota(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<AdminUserQuotaForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let body = serde_json::json!({
        "mail_quota_mb": f.mail_quota_mb,
        "drive_quota_gb": f.drive_quota_gb,
        "max_recipients_day": f.max_recipients_day,
        "max_attach_mb": f.max_attach_mb,
    });
    let _ = patch_json::<serde_json::Value>(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/quota"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await;
    Ok(Redirect::to(&format!("/admin/users/{id}?flash=Quotas+salvas")).into_response())
}
async fn admin_user_revoke_sessions(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/sessions/revoke"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok(Redirect::to(&format!("/admin/users/{id}?flash=Sessões+revogadas")).into_response())
}

#[derive(Deserialize)]
struct AdminInviteForm {
    email: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    role: String,
}

async fn admin_users_invite(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<AdminInviteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let body =
        serde_json::json!({"email": f.email, "display_name": f.display_name, "role": f.role});
    let _ = post_json(
        &st,
        &st.backends.auth,
        "/api/v1/admin/users/invite",
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await;
    Ok(Redirect::to("/admin/users?flash=Convite+enviado").into_response())
}

#[derive(Deserialize)]
struct AdminRoleForm {
    role: String,
}

async fn admin_users_set_role(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<AdminRoleForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let body = serde_json::json!({"role": f.role});
    let _ = patch_json(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/role"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await;
    Ok(Redirect::to("/admin/users?flash=Papel+atualizado").into_response())
}

async fn admin_users_suspend(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/suspend"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok(Redirect::to("/admin/users?flash=Usuário+suspenso").into_response())
}

async fn admin_users_activate(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/activate"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok(Redirect::to("/admin/users?flash=Usuário+reativado").into_response())
}

async fn admin_users_reset_password(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/users/{id}/reset-password"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok(Redirect::to("/admin/users?flash=Email+de+redefinição+enviado").into_response())
}

// ─── /admin/tenants ───────────────────────────────────────────────────────────

async fn admin_tenants_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let tenants = get_json::<Vec<AdminTenant>>(
        &st,
        &st.backends.auth,
        "/api/v1/admin/tenants",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let flash = extract_flash(&uri);
    Ok(askama_axum::IntoResponse::into_response(AdminTenantsTpl {
        me,
        tenants,
        flash,
    }))
}

#[derive(Deserialize)]
struct AdminTenantForm {
    name: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    quota_gb: i64,
}

async fn admin_tenants_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<AdminTenantForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let body = serde_json::json!({"name": f.name, "domain": f.domain, "quota_gb": f.quota_gb});
    let _ = post_json(
        &st,
        &st.backends.auth,
        "/api/v1/admin/tenants",
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await;
    Ok(Redirect::to("/admin/tenants?flash=Tenant+criado").into_response())
}

async fn admin_tenants_toggle(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.auth,
        &format!("/api/v1/admin/tenants/{id}/toggle"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok(Redirect::to("/admin/tenants").into_response())
}

// ─── /admin/monitoring ────────────────────────────────────────────────────────

async fn admin_monitoring_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    Ok(askama_axum::IntoResponse::into_response(
        AdminMonitoringTpl { me },
    ))
}

// ─── /admin/audit ─────────────────────────────────────────────────────────────

async fn admin_audit_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let events = get_json::<Vec<AuditEvent>>(
        &st,
        &st.backends.auth,
        "/api/v1/admin/audit?limit=200",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(AdminAuditTpl {
        me,
        events,
    }))
}

// ─── /admin/config ────────────────────────────────────────────────────────────

fn extract_flash(uri: &Uri) -> Option<String> {
    let q = uri.query().unwrap_or("");
    for part in q.split('&') {
        if let Some(v) = part.strip_prefix("flash=") {
            return Some(v.replace('+', " ").replace("%20", " "));
        }
    }
    None
}

async fn admin_config_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let config = get_json::<serde_json::Value>(
        &st,
        &st.backends.auth,
        "/api/v1/admin/config",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let cfg = AdminConfig {
        platform_name: config["platform_name"]
            .as_str()
            .unwrap_or("Expresso")
            .to_string(),
        logo_url: config["logo_url"].as_str().unwrap_or("").to_string(),
        accent_color: config["accent_color"]
            .as_str()
            .unwrap_or("#0ea5e9")
            .to_string(),
        mail_domain: config["mail_domain"].as_str().unwrap_or("").to_string(),
        mail_quota_mb: config["mail_quota_mb"].as_i64().unwrap_or(1024),
        allow_external_relay: config["allow_external_relay"].as_bool().unwrap_or(false),
        jitsi_domain: config["jitsi_domain"].as_str().unwrap_or("").to_string(),
        jitsi_recording: config["jitsi_recording"].as_bool().unwrap_or(false),
        drive_quota_gb: config["drive_quota_gb"].as_i64().unwrap_or(10),
        blocked_extensions: config["blocked_extensions"]
            .as_str()
            .unwrap_or("exe,bat,ps1")
            .to_string(),
        require_mfa: config["require_mfa"].as_bool().unwrap_or(false),
        session_hours: config["session_hours"].as_i64().unwrap_or(24),
        allowed_cidrs: config["allowed_cidrs"]
            .as_str()
            .unwrap_or("0.0.0.0/0")
            .to_string(),
    };
    let flash = extract_flash(&uri);
    Ok(askama_axum::IntoResponse::into_response(AdminConfigTpl {
        me,
        config: cfg,
        flash,
    }))
}

#[derive(Deserialize)]
struct AdminConfigForm {
    #[serde(default)]
    platform_name: String,
    #[serde(default)]
    logo_url: String,
    #[serde(default)]
    accent_color: String,
    #[serde(default)]
    mail_domain: String,
    #[serde(default)]
    mail_quota_mb: i64,
    #[serde(default)]
    allow_external_relay: Option<String>,
    #[serde(default)]
    jitsi_domain: String,
    #[serde(default)]
    jitsi_secret: String,
    #[serde(default)]
    jitsi_recording: Option<String>,
    #[serde(default)]
    drive_quota_gb: i64,
    #[serde(default)]
    blocked_extensions: String,
    #[serde(default)]
    require_mfa: Option<String>,
    #[serde(default)]
    session_hours: i64,
    #[serde(default)]
    allowed_cidrs: String,
}

async fn admin_config_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<AdminConfigForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let body = serde_json::json!({
        "platform_name":       f.platform_name,
        "logo_url":            f.logo_url,
        "accent_color":        f.accent_color,
        "mail_domain":         f.mail_domain,
        "mail_quota_mb":       f.mail_quota_mb,
        "allow_external_relay": f.allow_external_relay.is_some(),
        "jitsi_domain":        f.jitsi_domain,
        "jitsi_secret":        if f.jitsi_secret.is_empty() { serde_json::Value::Null } else { f.jitsi_secret.into() },
        "jitsi_recording":     f.jitsi_recording.is_some(),
        "drive_quota_gb":      f.drive_quota_gb,
        "blocked_extensions":  f.blocked_extensions,
        "require_mfa":         f.require_mfa.is_some(),
        "session_hours":       f.session_hours,
        "allowed_cidrs":       f.allowed_cidrs,
    });
    let _ = put_json(
        &st,
        &st.backends.auth,
        "/api/v1/admin/config",
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await;
    Ok(Redirect::to("/admin/config?flash=Configurações+salvas").into_response())
}

async fn admin_api_stats(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "{}").into_response());
    }
    let (t, u) = ctx_of(&me);
    let stats = get_json::<serde_json::Value>(
        &st,
        &st.backends.auth,
        "/api/v1/admin/stats",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!({}));
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&stats).unwrap_or_default(),
    )
        .into_response())
}

#[derive(Deserialize)]
struct AdminAuditQuery {
    since: Option<String>,
}

async fn admin_api_audit(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<AdminAuditQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "[]").into_response());
    }
    let (t, u) = ctx_of(&me);
    let path = if let Some(ref since) = q.since {
        format!(
            "/api/v1/admin/audit?since={}",
            utf8_percent_encode(since, NON_ALPHANUMERIC)
        )
    } else {
        "/api/v1/admin/audit".into()
    };
    let events =
        get_json::<serde_json::Value>(&st, &st.backends.auth, &path, &headers, Some((&t, &u)))
            .await?
            .unwrap_or(serde_json::json!([]));
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&events).unwrap_or_default(),
    )
        .into_response())
}

async fn admin_api_domain_quotas(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "[]").into_response());
    }
    let (t, u) = ctx_of(&me);
    let quotas = get_json::<serde_json::Value>(
        &st,
        &st.backends.auth,
        "/api/v1/admin/domain-quotas",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!([]));
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&quotas).unwrap_or_default(),
    )
        .into_response())
}

async fn admin_api_domain_quotas_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: axum::body::Bytes,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "").into_response());
    }
    let (t, u) = ctx_of(&me);
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::json!([]));
    let _ = put_json(
        &st,
        &st.backends.auth,
        "/api/v1/admin/domain-quotas",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await;
    Ok((StatusCode::OK, "").into_response())
}

async fn admin_api_smtp_queue(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "").into_response());
    }
    let (t, u) = ctx_of(&me);
    let data: serde_json::Value = get_json(
        &st,
        &st.backends.mail,
        "/api/v1/admin/smtp-queue",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!({"queued":0,"deferred":0,"failed":0,"items":[]}));
    Ok(axum::Json(data).into_response())
}

async fn admin_api_smtp_queue_flush(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_json(
        &st,
        &st.backends.mail,
        "/api/v1/admin/smtp-queue/flush",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({}),
    )
    .await;
    Ok((StatusCode::OK, "").into_response())
}
