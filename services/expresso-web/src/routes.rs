//! HTTP routes — SSR pages.

use axum::{
    body::Bytes,
    extract::{Form, Path, Query, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post, put},
    Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::Deserialize;

use crate::{
    error::WebResult,
    ical::{
        booked_resources_from_ical, build_vcalendar, categories_from_ical, to_rfc3339,
        valarm_minutes, EventForm,
    },
    templates::{
        AclRow, ActivityRow, AddrbookShareTpl, AddressBook, AdminAuditTpl, AdminConfig,
        AdminConfigTpl, AdminDlqTpl, AdminLoginEvent, AdminMonitoringTpl, AdminResourcesTpl,
        AdminRetentionTpl, AdminTenant, AdminTenantUsageTpl, AdminTenantsTpl, AdminUser,
        AdminUserDetailTpl, AdminUsersTpl, ApiTokenRow, ArchiveRow, ArchiveStatRow,
        ArchiveTagHistRow, AuditEvent, BulkDeleteEventRow, Calendar, CalendarBulkDeleteTpl,
        CalendarConflictsTpl, CalendarCountersTpl, CalendarDayTpl, CalendarHistogramTpl,
        CalendarMonthTpl, CalendarShareTpl, CalendarTpl, CalendarWeekTpl, ChatAttachment,
        ChatChannel, ChatMessage, ChatTpl, ComplianceArchiveTpl, ComplianceStatsTpl,
        ComplianceTagsTpl, ConflictPairRow, Contact, ContactActivityTpl, ContactAddressRow,
        ContactDiffTpl, ContactEmailRow, ContactFormTpl, ContactGroup, ContactGroupDetailTpl,
        ContactGroupsTpl, ContactVersionRow, ContactVersionsTpl, ContactsTpl, CounterRow,
        DayColumn, DelegationRaw, DelegationView, DelegationsTpl, DlqEntry, DlqKindCount,
        DriveActivityTpl, DriveCommentRow, DriveCommentsTpl, DriveContentHit,
        DriveContentSearchTpl, DriveEditTpl, DriveFile, DriveFileTag, DrivePreviewTpl, DriveQuota,
        DriveShareTpl, DriveStarredTpl, DriveTagFilesTpl, DriveTagStat, DriveTagsTpl, DriveTpl,
        DriveTrashTpl, DriveVersionsTpl, Event, EventFormTpl, FlagPreset, FlowEditTpl, FlowRuleRow,
        FlowsTpl, Folder, FreeBusyRow, FreeBusyTpl, GalContact, HistogramBar, HomeDriveFile,
        HomeEvent, HomeTpl, LoginTpl, MailAlias, MailComposeTpl, MailListTpl, MailSearchTpl,
        MailSnoozedTpl, MailThreadTpl, Me, MeTpl, MeetParticipant, MeetRoom, MeetRoomTpl,
        MeetScheduleTpl, MeetTpl, MessageDetail, MessageListItem, MfaFactorRow, MonthCell, Note,
        NoteTagStat, NoteVersionRow, NoteVersionsTpl, Notebook, NotesActivityTpl, NotesSharedTpl,
        NotesTagsTpl, NotesTpl, Resource, RetentionPolicyRow, SearchFacet, SearchGroup, SearchHit,
        SearchTpl, SecurityTpl, SettingsTokensTpl, SettingsTpl, ShareRow, SharedNoteRow,
        SnoozedRow, TagPairRow, TaskRow, TasksTpl, TenantUsageRow, VersionRow, WorkingHour,
    },
    upstream::{
        delete_at, delete_json, get_bytes, get_json, patch_json, post_body, post_body_json,
        post_empty, post_json, put_body, put_json, put_json_body,
    },
    vcard::{build_vcard, ContactForm},
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
        .route("/mail/send-undo", post(mail_send_undo_action))
        .route("/mail/:id/cancel-send", post(mail_cancel_send_action))
        .route("/mail/rules", get(mail_rules_page).post(mail_rules_save))
        .route("/mail/thread/:tid", get(mail_thread_page))
        .route("/mail/thread/:tid/mute", post(mail_thread_mute_action))
        .route("/mail/thread/:tid/pin", post(mail_thread_pin_action))
        .route("/mail/:id", get(mail_detail_page))
        .route("/drive", get(drive_page))
        .route("/drive/trash", get(drive_trash_page))
        .route("/drive/starred", get(drive_starred_page))
        .route("/drive/:id/star", post(drive_star_action))
        .route("/drive/:id/unstar", post(drive_unstar_action))
        .route("/drive/:id/lock", post(drive_lock_action))
        .route("/drive/:id/unlock", post(drive_unlock_action))
        .route("/drive/:id/expiry", post(drive_expiry_action))
        .route("/drive/:id/copy", post(drive_copy_action))
        .route("/drive/upload", post(drive_upload_action))
        .route("/drive/:id/trash", post(drive_trash_action))
        .route("/drive/:id/restore", post(drive_restore_action))
        .route("/drive/:id/purge", post(drive_purge_action))
        .route(
            "/drive/:id/share",
            get(drive_share_page).post(drive_share_create),
        )
        .route("/drive/:id/share/:sid/revoke", post(drive_share_revoke))
        .route(
            "/drive/:id/comments",
            get(drive_comments_page).post(drive_comment_create),
        )
        .route(
            "/drive/:id/comments/:comment_id/delete",
            post(drive_comment_delete),
        )
        .route("/drive/:id/versions", get(drive_versions_page))
        .route(
            "/drive/:id/versions/:vno/restore",
            post(drive_version_restore),
        )
        .route("/drive/:id/activity", get(drive_activity_page))
        .route("/drive/:id/tags/add", post(drive_tag_add_action))
        .route("/drive/:id/tags/remove", post(drive_tag_remove_action))
        .route("/drive/:id/preview", get(drive_preview_page))
        .route("/drive/:id/edit", get(drive_edit_page))
        .route("/calendar", get(calendar_page))
        .route("/calendar/freebusy", get(freebusy_page))
        .route(
            "/calendar/bulk-delete",
            get(calendar_bulk_delete_page).post(calendar_bulk_delete_action),
        )
        .route("/calendar/bulk-move", post(calendar_bulk_move_action))
        .route("/calendar/bulk-status", post(calendar_bulk_status_action))
        .route(
            "/calendar/bulk-location",
            post(calendar_bulk_location_action),
        )
        .route("/calendar/bulk-summary", post(calendar_bulk_summary_action))
        .route("/calendar/conflicts", get(calendar_conflicts_page))
        .route("/calendar/histogram", get(calendar_histogram_page))
        .route("/calendar/counters", get(calendar_counters_page))
        .route(
            "/calendar/counters/:id/:action",
            post(calendar_counter_action),
        )
        .route(
            "/calendar/resources/:id/conflicts",
            get(resource_conflicts_api),
        )
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
            "/calendar/:cal_id/events/:id/send-invite",
            post(event_send_invite_action),
        )
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
        .route("/contacts/:book_id/:id/photo", get(contact_photo))
        .route(
            "/contacts/:book_id/:id/activity",
            get(contact_activity_page),
        )
        .route(
            "/contacts/:book_id/:id/versions",
            get(contact_versions_page),
        )
        .route(
            "/contacts/:book_id/:id/versions/:vno/restore",
            post(contact_version_restore_action),
        )
        .route(
            "/contacts/:book_id/:id/diff/:from/:to",
            get(contact_diff_page),
        )
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
        .route("/contacts/import-csv", post(contacts_import_csv))
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
        .route("/mail/:id/apply-preset", post(mail_apply_preset_action))
        .route("/mail/:id/read-receipt", post(mail_read_receipt_action))
        // mail flow rules (automation)
        .route("/notes/export.json", get(notes_export_json))
        .route(
            "/mail/folders/:id/export.mbox",
            get(mail_folder_export_mbox),
        )
        .route("/flows", get(flows_page).post(flow_create_action))
        .route("/flows/reorder", post(flow_reorder_action))
        .route("/compliance/archive", get(compliance_archive_page))
        .route("/compliance/archive/export", get(compliance_archive_export))
        .route("/compliance/archive/hold", post(compliance_hold_action))
        .route("/compliance/archive/unhold", post(compliance_unhold_action))
        .route("/compliance/stats", get(compliance_stats_page))
        .route("/compliance/tags", get(compliance_tags_page))
        .route(
            "/compliance/tags/rename",
            post(compliance_tag_rename_action),
        )
        .route("/compliance/tags/merge", post(compliance_tag_merge_action))
        .route(
            "/compliance/tags/rename-history/:id/undo",
            post(compliance_tag_rename_undo),
        )
        .route(
            "/compliance/tags/merge-history/:id/undo",
            post(compliance_tag_merge_undo),
        )
        .route(
            "/compliance/archive/:id/tags",
            get(compliance_tags_get).post(compliance_tag_add),
        )
        .route(
            "/compliance/archive/:id/tags/:tag/delete",
            post(compliance_tag_remove),
        )
        .route(
            "/flows/:id/edit",
            get(flow_edit_page).post(flow_edit_action),
        )
        .route("/flows/:id/toggle", post(flow_toggle_action))
        .route("/flows/:id/delete", post(flow_delete_action))
        .route("/mail/:id/move", post(mail_move_action))
        .route("/mail/:id/delete", post(mail_delete_action))
        .route("/mail/snoozed", get(mail_snoozed_page))
        .route("/mail/drafts", post(mail_draft_save))
        .route("/mail/drafts/:id", put(mail_draft_update))
        .route("/mail/:id/snooze", post(mail_snooze_action))
        .route("/mail/:id/unsnooze", post(mail_unsnooze_action))
        // mail folder management
        .route("/mail/folders/create", post(mail_folder_create_action))
        .route("/mail/folders/rename", post(mail_folder_rename_action))
        .route("/mail/folders/delete", post(mail_folder_delete_action))
        // drive extras
        .route("/drive/search", get(drive_search_page))
        .route("/drive/content-search", get(drive_content_search_page))
        .route("/drive/new-folder", post(drive_mkdir_action))
        .route("/drive/:id/rename", post(drive_rename_action))
        .route("/drive/:id/move", post(drive_move_action))
        .route("/drive/bulk-move", post(drive_bulk_move_action))
        .route("/drive/bulk-copy", post(drive_bulk_copy_action))
        .route("/drive/tags", get(drive_tags_page))
        .route("/drive/tags/:tag", get(drive_tag_files_page))
        // contacts extras
        .route("/contacts/gal", get(contacts_gal_page))
        .route("/contacts/gal/save", post(contacts_gal_save_action))
        // chat / meet
        .route("/chat", get(chat_page))
        .route("/chat/channels", post(chat_create_channel))
        .route("/chat/channels/:cid", get(chat_channel_page))
        .route("/chat/channels/:cid/send", post(chat_send_message))
        .route("/chat/channels/:cid/poll", get(chat_poll_messages))
        .route(
            "/chat/channels/:cid/attachments/:aid/download",
            get(chat_attachment_download),
        )
        .route("/chat/channels/:cid/mark-read", post(chat_mark_read))
        .route("/chat/channels/:cid/typing", post(chat_typing))
        .route(
            "/chat/channels/:cid/presence/heartbeat",
            post(chat_presence_heartbeat),
        )
        .route("/chat/channels/:cid/presence", get(chat_presence_roster))
        .route(
            "/chat/channels/:cid/messages/:mid/react",
            post(chat_react_message),
        )
        .route(
            "/chat/channels/:cid/messages/:mid/edits",
            get(chat_message_edits),
        )
        .route(
            "/chat/channels/:cid/messages/:mid",
            axum::routing::patch(chat_edit_message).delete(chat_delete_message),
        )
        .route(
            "/chat/channels/:cid/members/invite",
            post(chat_invite_member),
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
        .route(
            "/meet/:id/recordings/:rec_id/delete",
            post(meet_recording_delete_api),
        )
        .route("/meet/:id/recording/start", post(meet_recording_start_api))
        .route("/meet/:id/recording/stop", post(meet_recording_stop_api))
        .route(
            "/meet/:id/polls",
            get(meet_polls_list_api).post(meet_poll_create_api),
        )
        .route("/meet/:id/polls/:poll_id", get(meet_poll_get_api))
        .route("/meet/:id/polls/:poll_id/vote", post(meet_poll_vote_api))
        .route("/meet/:id/lobby", get(meet_lobby_list_api))
        .route(
            "/meet/:id/lobby/:user_id/approve",
            post(meet_lobby_approve_api),
        )
        .route("/meet/:id/lobby/:user_id/deny", post(meet_lobby_deny_api))
        .route("/meet/:id/transcripts", get(meet_transcripts_list_api))
        .route(
            "/meet/:id/transcripts/search",
            get(meet_transcripts_search_api),
        )
        .route(
            "/meet/:id/breakouts",
            get(meet_breakouts_list_api).post(meet_breakout_create_api),
        )
        .route(
            "/meet/:id/breakouts/:room_id/delete",
            post(meet_breakout_delete_api),
        )
        .route(
            "/meet/:id/breakouts/:room_id/participants",
            post(meet_breakout_assign_api).delete(meet_breakout_remove_api),
        )
        // tasks
        .route("/tasks", get(tasks_page))
        .route("/tasks/create", post(tasks_create_action))
        .route("/tasks/:id/complete", post(tasks_complete_action))
        .route("/tasks/:id/repeat", post(tasks_repeat_action))
        .route("/tasks/:id/delete", post(tasks_delete_action))
        .route("/notes", get(notes_page).post(notes_create_action))
        .route("/notes/tags", get(notes_tags_page))
        .route("/notes/tags/rename", post(notes_tag_rename_action))
        .route("/notes/tags/merge", post(notes_tag_merge_action))
        .route("/notes/notebooks", post(notes_notebook_create_action))
        .route(
            "/notes/notebooks/:id/rename",
            post(notes_notebook_rename_action),
        )
        .route(
            "/notes/notebooks/:id/delete",
            post(notes_notebook_delete_action),
        )
        .route("/notes/:id", post(notes_edit_action))
        .route("/notes/:id/delete", post(notes_delete_action))
        .route("/notes/shared", get(notes_shared_page))
        .route("/notes/:id/activity", get(notes_activity_page))
        .route(
            "/notes/:id/tags",
            get(notes_tags_get_api).post(notes_tags_set_api),
        )
        .route("/notes/:id/versions", get(notes_versions_page))
        .route(
            "/notes/:id/versions/:vno/restore",
            post(notes_version_restore_action),
        )
        // settings
        .route("/settings", get(settings_page))
        .route("/settings/profile", post(settings_profile_save))
        .route("/settings/signature", post(settings_signature_save))
        .route("/settings/autoreply", post(settings_autoreply_save))
        .route("/settings/notifications", post(settings_notifications_save))
        .route("/settings/filters", post(settings_filters_save))
        .route("/settings/filters/test", post(settings_filters_test))
        .route("/settings/working-hours", post(settings_working_hours_save))
        .route(
            "/settings/delegations",
            get(delegations_page).post(delegation_grant_action),
        )
        .route(
            "/settings/delegations/:id/revoke",
            post(delegation_revoke_action),
        )
        .route(
            "/settings/tokens",
            get(settings_tokens_page).post(settings_token_create),
        )
        .route("/settings/tokens/:id/revoke", post(settings_token_revoke))
        .route("/settings/aliases", post(settings_alias_create))
        .route("/settings/aliases/:id/toggle", post(settings_alias_toggle))
        .route("/settings/aliases/:id/delete", post(settings_alias_delete))
        .route("/settings/flag-presets", post(settings_flag_preset_create))
        .route(
            "/settings/flag-presets/:id/delete",
            post(settings_flag_preset_delete),
        )
        .route(
            "/settings/flag-presets/:id/edit",
            post(settings_flag_preset_edit),
        )
        // Service worker — must be served from the root scope (not /static).
        .route("/sw.js", get(service_worker_js))
        // WebPush registration (settings → notifications)
        .route("/notifications/vapid-key", get(notifications_vapid_key))
        .route(
            "/notifications/push/subscribe",
            post(notifications_push_subscribe),
        )
        .route(
            "/notifications/push/unsubscribe",
            post(notifications_push_unsubscribe),
        )
        // Notification bell tray (server-backed; static before :id)
        .route("/notifications/list", get(notifications_list_api))
        .route(
            "/notifications/read-all",
            post(notifications_read_all_action),
        )
        .route("/notifications/:id/read", post(notification_read_action))
        .route(
            "/notifications/:id/snooze",
            post(notification_snooze_action),
        )
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
        .route("/admin/tenants/:id/usage", get(admin_tenant_usage_page))
        .route("/admin/users/:id/impersonate", post(admin_user_impersonate))
        .route("/admin/users/:id/mfa/require", post(admin_user_mfa_require))
        .route(
            "/admin/users/:id/mfa/:cred/delete",
            post(admin_user_mfa_delete),
        )
        .route("/impersonate/end", post(impersonation_end))
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
        .route("/admin/dlq", get(admin_dlq_page))
        .route("/admin/dlq/purge", post(admin_dlq_purge))
        .route("/admin/dlq/retry-all", post(admin_dlq_retry_all))
        .route("/admin/dlq/:id/retry", post(admin_dlq_retry))
        .route("/admin/dlq/:id/delete", post(admin_dlq_delete))
        .route(
            "/admin/resources",
            get(admin_resources_page).post(admin_resource_create),
        )
        .route("/admin/resources/:id/delete", post(admin_resource_delete))
        .route("/admin/retention", get(admin_retention_page))
        .route(
            "/admin/retention/default",
            post(admin_retention_set_default),
        )
        .route(
            "/admin/retention/policies",
            post(admin_retention_policy_create),
        )
        .route(
            "/admin/retention/policies/:id/toggle",
            post(admin_retention_policy_toggle),
        )
        .route(
            "/admin/retention/policies/:id/delete",
            post(admin_retention_policy_delete),
        )
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

    // Tasks due today or overdue (pending only), from the default calendar.
    let tasks_due: Vec<TaskRow> = {
        let cal_id = default_calendar_id(&st, &headers, &t, &u).await;
        if cal_id.is_empty() {
            Vec::new()
        } else {
            let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC);
            let all = get_json::<Vec<TaskRow>>(
                &st,
                &st.backends.calendar,
                &format!("/api/v1/calendars/{enc}/tasks"),
                &headers,
                Some((&t, &u)),
            )
            .await?
            .unwrap_or_default();
            let now = time::OffsetDateTime::now_utc();
            let today_date = format!(
                "{:04}-{:02}-{:02}",
                now.year(),
                now.month() as u8,
                now.day()
            );
            all.into_iter()
                .filter(|task| {
                    !task.is_done()
                        && !task.due_date().is_empty()
                        && task.due_date() <= today_date.as_str()
                })
                .take(8)
                .collect()
        }
    };

    Ok(askama_axum::IntoResponse::into_response(HomeTpl {
        me,
        mail_unread,
        inbox_id,
        events,
        drive_files,
        chat_unread,
        tasks_due,
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
        flag_presets: Vec::new(),
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

    // Offer the user's flag presets as quick-apply buttons on the open message.
    let flag_presets = get_json::<Vec<FlagPreset>>(
        &st,
        &st.backends.mail,
        "/api/v1/mail/flag-presets",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

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
        flag_presets,
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

    let state = get_json::<ThreadState>(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/threads/{enc_tid}/state"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    Ok(askama_axum::IntoResponse::into_response(MailThreadTpl {
        me,
        folders,
        thread_id: tid,
        messages,
        subject,
        muted: state.muted,
        pinned: state.pinned,
    }))
}

#[derive(Deserialize, Default)]
struct ThreadState {
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    pinned: bool,
}

/// PUT/DELETE proxy for a thread's mute or pin flag. `on=true` → PUT (set),
/// `on=false` → DELETE (clear). `flag` is "mute" or "pin".
async fn mail_thread_flag(
    st: &AppState,
    headers: &HeaderMap,
    me: &Me,
    tid: &str,
    flag: &str,
    on: bool,
) -> WebResult<u16> {
    let (t, u) = ctx_of(me);
    let enc = utf8_percent_encode(tid, NON_ALPHANUMERIC);
    let path = format!("/api/v1/mail/threads/{enc}/{flag}");
    if on {
        put_json(st, &st.backends.mail, &path, headers, Some((&t, &u)), &()).await
    } else {
        delete_at(st, &st.backends.mail, &path, headers, Some((&t, &u))).await
    }
}

#[derive(Deserialize)]
struct ThreadFlagForm {
    #[serde(default)]
    on: bool,
}

/// POST /mail/thread/:tid/mute — toggle mute (form field `on`=true|false).
async fn mail_thread_mute_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(tid): Path<String>,
    Form(f): Form<ThreadFlagForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let status = mail_thread_flag(&st, &headers, &me, &tid, "mute", f.on).await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /mail/thread/:tid/pin — toggle pin (form field `on`=true|false).
async fn mail_thread_pin_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(tid): Path<String>,
    Form(f): Form<ThreadFlagForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let status = mail_thread_flag(&st, &headers, &me, &tid, "pin", f.on).await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
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

/// GET /drive/starred — the user's server-backed favorites, across all folders.
async fn drive_starred_page(
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
        "/api/v1/drive/starred",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(DriveStarredTpl {
        me,
        files,
    }))
}

/// POST /drive/:id/star — mark a file as a server-backed favorite.
async fn drive_star_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/star"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
}

/// POST /drive/:id/unstar — remove a file from favorites.
async fn drive_unstar_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/star"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
}

/// POST /drive/:id/lock — acquire an exclusive edit lock (backend: owner or
/// first caller; 409 if held by another user).
async fn drive_lock_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/lock"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
}

/// POST /drive/:id/unlock — release the lock (backend DELETE; 403 unless held by
/// the caller).
async fn drive_unlock_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/lock"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
}

#[derive(Deserialize)]
struct ExpiryForm {
    /// RFC3339 instant (e.g. "2026-07-01T00:00:00Z"); empty clears the expiry.
    #[serde(default)]
    expires_at: String,
}

/// POST /drive/:id/expiry — set or clear a file's auto-delete date (owner only,
/// backend enforces). Empty `expires_at` clears it (PATCH with `null`).
async fn drive_expiry_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<ExpiryForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let exp = f.expires_at.trim();
    let body = if exp.is_empty() {
        serde_json::json!({ "expires_at": null })
    } else {
        serde_json::json!({ "expires_at": exp })
    };
    let status = patch_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/expiry"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
}

#[derive(Deserialize)]
struct DriveCopyForm {
    #[serde(default)]
    name: String,
}

/// POST /drive/:id/copy — shallow-copy a single file (optional new name; backend
/// defaults to "<name> (cópia)"). Copies into the same parent.
async fn drive_copy_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<DriveCopyForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let name = f.name.trim();
    let body = if name.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({ "name": name })
    };
    let status = post_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/copy"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
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

#[derive(Deserialize)]
struct FreeBusyQuery {
    attendees: Option<String>,
    date: Option<String>,
}

/// Extract "HH:MM" from an RFC3339 instant (chars 11..16), best-effort.
fn hhmm_of_rfc3339(s: &str) -> &str {
    s.get(11..16).unwrap_or(s)
}

/// GET /calendar/freebusy — look up attendees' busy intervals for a day so the
/// organizer can eyeball free slots. Proxies the calendar freebusy endpoint.
#[derive(Deserialize)]
struct CounterProposal {
    id: String,
    event_id: String,
    attendee_email: String,
    #[serde(default)]
    proposed_dtstart: Option<String>,
    #[serde(default)]
    proposed_dtend: Option<String>,
    #[serde(default)]
    comment: Option<String>,
}

fn fmt_dt(s: Option<String>) -> String {
    s.map(|v| v.replace('T', " ").chars().take(16).collect())
        .unwrap_or_default()
}

/// GET /calendar/counters — pending COUNTER proposals (attendees suggesting a
/// different time), each with accept (re-times the event) / reject actions.
async fn calendar_counters_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let proposals = get_json::<Vec<CounterProposal>>(
        &st,
        &st.backends.calendar,
        "/api/v1/scheduling/counters",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let rows = proposals
        .into_iter()
        .map(|p| CounterRow {
            id: p.id,
            event_id: p.event_id,
            attendee_email: p.attendee_email,
            proposed_start: fmt_dt(p.proposed_dtstart),
            proposed_end: fmt_dt(p.proposed_dtend),
            comment: p.comment.unwrap_or_default(),
        })
        .collect();
    Ok(askama_axum::IntoResponse::into_response(
        CalendarCountersTpl { me, rows },
    ))
}

/// POST /calendar/counters/:id/:action — accept or reject a proposal. `action`
/// is "accept" or "reject"; anything else is rejected as a bad request.
async fn calendar_counter_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, action)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if action != "accept" && action != "reject" {
        return Ok((StatusCode::BAD_REQUEST, "invalid action").into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/scheduling/counters/{enc}/{action}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

#[derive(Deserialize)]
struct HistogramQuery {
    #[serde(default)]
    cal_id: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
}

#[derive(Deserialize)]
struct HistogramPoint {
    #[serde(default)]
    ts: Option<String>,
    #[serde(default)]
    count: i64,
}

/// Trim an rfc3339 `ts` to a bucket label: month → "YYYY-MM", else "YYYY-MM-DD".
fn histogram_label(ts: &str, bucket: &str) -> String {
    let take = if bucket == "month" { 7 } else { 10 };
    ts.chars().take(take).collect()
}

/// GET /calendar/histogram?cal_id=&from=&to=&bucket= — event-activity histogram
/// (counts per day/week/month) rendered as CSS bars. Read-only analytics.
async fn calendar_histogram_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<HistogramQuery>,
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
    let default_cal = calendars
        .iter()
        .find(|c| c.is_default)
        .or_else(|| calendars.first())
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let cal_id = q.cal_id.filter(|s| !s.is_empty()).unwrap_or(default_cal);
    let from = q.from.unwrap_or_default();
    let to = q.to.unwrap_or_default();
    let bucket = match q.bucket.as_deref() {
        Some("week") => "week",
        Some("month") => "month",
        _ => "day",
    }
    .to_string();

    let mut bars = Vec::new();
    let mut total = 0i64;
    let queried = !cal_id.is_empty() && from.len() == 10 && to.len() == 10 && from <= to;
    if queried {
        let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC);
        let f = format!("{from}T00:00:00Z");
        let tt = format!("{to}T23:59:59Z");
        let resp = get_json::<serde_json::Value>(
            &st,
            &st.backends.calendar,
            &format!("/api/v1/calendars/{enc}/events-histogram?from={f}&to={tt}&bucket={bucket}"),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        let points: Vec<HistogramPoint> = resp
            .get("series")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| serde_json::from_value(p.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let max = points.iter().map(|p| p.count).max().unwrap_or(0).max(1);
        for p in points {
            total += p.count;
            let pct = ((p.count * 100) / max) as u32;
            bars.push(HistogramBar {
                label: p
                    .ts
                    .as_deref()
                    .map(|s| histogram_label(s, &bucket))
                    .unwrap_or_default(),
                count: p.count,
                pct,
            });
        }
    }
    Ok(askama_axum::IntoResponse::into_response(
        CalendarHistogramTpl {
            me,
            calendars,
            cal_id,
            from,
            to,
            bucket,
            bars,
            total,
            queried,
        },
    ))
}

#[derive(Deserialize)]
struct ConflictsQuery {
    #[serde(default)]
    cal_id: Option<String>,
    #[serde(default)]
    date: Option<String>,
}

#[derive(Deserialize)]
struct ConflictPairJson {
    #[serde(default)]
    a_summary: Option<String>,
    #[serde(default)]
    a_dtstart: Option<String>,
    #[serde(default)]
    a_dtend: Option<String>,
    #[serde(default)]
    b_summary: Option<String>,
    #[serde(default)]
    b_dtstart: Option<String>,
    #[serde(default)]
    b_dtend: Option<String>,
}

/// "HH:MM–HH:MM" from two rfc3339 instants (blank parts skipped).
fn span_label(start: &Option<String>, end: &Option<String>) -> String {
    let s = start.as_deref().map(hhmm_of_rfc3339).unwrap_or_default();
    let e = end.as_deref().map(hhmm_of_rfc3339).unwrap_or_default();
    format!("{s}–{e}")
}

/// GET /calendar/conflicts?cal_id=&date= — double-booking detector. Lists pairs
/// of events on a calendar that overlap in time on the chosen day.
async fn calendar_conflicts_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<ConflictsQuery>,
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
    // Default the picker to the user's default calendar (or the first).
    let default_cal = calendars
        .iter()
        .find(|c| c.is_default)
        .or_else(|| calendars.first())
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let cal_id = q.cal_id.filter(|s| !s.is_empty()).unwrap_or(default_cal);
    let date = q.date.unwrap_or_default();

    let mut pairs = Vec::new();
    let queried = !cal_id.is_empty() && date.len() == 10;
    if queried {
        let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC);
        let from = format!("{date}T00:00:00Z");
        let to = format!("{date}T23:59:59Z");
        let resp = get_json::<serde_json::Value>(
            &st,
            &st.backends.calendar,
            &format!("/api/v1/calendars/{enc}/events-conflicts?from={from}&to={to}"),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        if let Some(arr) = resp.get("conflicts").and_then(|v| v.as_array()) {
            for c in arr {
                if let Ok(p) = serde_json::from_value::<ConflictPairJson>(c.clone()) {
                    pairs.push(ConflictPairRow {
                        a_summary: p.a_summary.unwrap_or_else(|| "(sem título)".into()),
                        a_when: span_label(&p.a_dtstart, &p.a_dtend),
                        b_summary: p.b_summary.unwrap_or_else(|| "(sem título)".into()),
                        b_when: span_label(&p.b_dtstart, &p.b_dtend),
                    });
                }
            }
        }
    }
    Ok(askama_axum::IntoResponse::into_response(
        CalendarConflictsTpl {
            me,
            calendars,
            cal_id,
            date,
            pairs,
            queried,
        },
    ))
}

#[derive(Deserialize)]
struct BulkDeleteQuery {
    #[serde(default)]
    cal_id: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

#[derive(Deserialize)]
struct BulkDeleteEventJson {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    dtstart: Option<String>,
    #[serde(default)]
    rrule: Option<String>,
}

/// Preview cap: list at most this many events before the range delete.
const BULK_DELETE_PREVIEW_CAP: i64 = 200;

/// GET /calendar/bulk-delete?cal_id=&from=&to= — preview events in a date range
/// (read-only) so the user can confirm before deleting them in bulk.
async fn calendar_bulk_delete_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<BulkDeleteQuery>,
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
    let default_cal = calendars
        .iter()
        .find(|c| c.is_default)
        .or_else(|| calendars.first())
        .map(|c| c.id.clone())
        .unwrap_or_default();
    let cal_id = q.cal_id.filter(|s| !s.is_empty()).unwrap_or(default_cal);
    let from = q.from.unwrap_or_default();
    let to = q.to.unwrap_or_default();

    let mut events = Vec::new();
    let mut truncated = false;
    let previewed = !cal_id.is_empty() && from.len() == 10 && to.len() == 10 && from <= to;
    if previewed {
        let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC);
        let after = format!("{from}T00:00:00Z");
        let before = format!("{to}T23:59:59Z");
        let cap = BULK_DELETE_PREVIEW_CAP + 1;
        let resp = get_json::<serde_json::Value>(
            &st,
            &st.backends.calendar,
            &format!(
                "/api/v1/calendars/{enc}/events-by-range?after={after}&before={before}&limit={cap}"
            ),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        if let Some(arr) = resp.get("events").and_then(|v| v.as_array()) {
            for e in arr.iter().take(BULK_DELETE_PREVIEW_CAP as usize) {
                if let Ok(ev) = serde_json::from_value::<BulkDeleteEventJson>(e.clone()) {
                    events.push(BulkDeleteEventRow {
                        summary: ev.summary.unwrap_or_else(|| "(sem título)".into()),
                        when: ev
                            .dtstart
                            .map(|s| s.replace('T', " ").chars().take(16).collect())
                            .unwrap_or_default(),
                        recurring: ev.rrule.is_some(),
                    });
                }
            }
            truncated = arr.len() as i64 > BULK_DELETE_PREVIEW_CAP;
        }
    }
    Ok(askama_axum::IntoResponse::into_response(
        CalendarBulkDeleteTpl {
            me,
            calendars,
            cal_id,
            from,
            to,
            events,
            previewed,
            truncated,
        },
    ))
}

#[derive(Deserialize)]
struct BulkDeleteForm {
    cal_id: String,
    from: String,
    to: String,
}

/// POST /calendar/bulk-delete — delete every event in the [from, to] range on a
/// calendar (write-gated by the backend), then back to the preview.
async fn calendar_bulk_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<BulkDeleteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    if f.cal_id.is_empty() || f.from.len() != 10 || f.to.len() != 10 || f.from > f.to {
        return Ok((StatusCode::BAD_REQUEST, "intervalo inválido").into_response());
    }
    let enc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    let after = format!("{}T00:00:00Z", f.from);
    let before = format!("{}T23:59:59Z", f.to);
    let _ = post_empty(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/events-bulk-delete?from={after}&to={before}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    let cenc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    Ok(Redirect::to(&format!(
        "/calendar/bulk-delete?cal_id={cenc}&from={}&to={}",
        f.from, f.to
    ))
    .into_response())
}

#[derive(Deserialize)]
struct CalBulkMoveForm {
    cal_id: String,
    dst: String,
    from: String,
    to: String,
}

/// POST /calendar/bulk-move — move every event in the [from, to] range from one
/// calendar to another (backend write-gated on both). Back to the preview.
async fn calendar_bulk_move_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<CalBulkMoveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    if f.cal_id.is_empty()
        || f.dst.is_empty()
        || f.dst == f.cal_id
        || f.from.len() != 10
        || f.to.len() != 10
        || f.from > f.to
    {
        return Ok((StatusCode::BAD_REQUEST, "parâmetros inválidos").into_response());
    }
    let enc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    let dst = utf8_percent_encode(&f.dst, NON_ALPHANUMERIC);
    let after = format!("{}T00:00:00Z", f.from);
    let before = format!("{}T23:59:59Z", f.to);
    let _ = patch_json(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{enc}/events-by-range/move?after={after}&before={before}&dst={dst}"
        ),
        &headers,
        Some((&t, &u)),
        &(),
    )
    .await?;
    let cenc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    Ok(Redirect::to(&format!(
        "/calendar/bulk-delete?cal_id={cenc}&from={}&to={}",
        f.from, f.to
    ))
    .into_response())
}

#[derive(Deserialize)]
struct BulkStatusForm {
    cal_id: String,
    status: String,
    from: String,
    to: String,
}

/// POST /calendar/bulk-status — set the STATUS of every event in the [from, to]
/// range (CONFIRMED|TENTATIVE|CANCELLED) without deleting them. Back to preview.
async fn calendar_bulk_status_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<BulkStatusForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = f.status.trim();
    let valid = matches!(status, "CONFIRMED" | "TENTATIVE" | "CANCELLED");
    if f.cal_id.is_empty() || !valid || f.from.len() != 10 || f.to.len() != 10 || f.from > f.to {
        return Ok((StatusCode::BAD_REQUEST, "parâmetros inválidos").into_response());
    }
    let enc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    let after = format!("{}T00:00:00Z", f.from);
    let before = format!("{}T23:59:59Z", f.to);
    let _ = patch_json(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{enc}/events-by-range/set-status?after={after}&before={before}&status={status}"
        ),
        &headers,
        Some((&t, &u)),
        &(),
    )
    .await?;
    let cenc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    Ok(Redirect::to(&format!(
        "/calendar/bulk-delete?cal_id={cenc}&from={}&to={}",
        f.from, f.to
    ))
    .into_response())
}

#[derive(Deserialize)]
struct BulkLocationForm {
    cal_id: String,
    #[serde(default)]
    location: String,
    from: String,
    to: String,
}

/// POST /calendar/bulk-location — set (or clear) the LOCATION of every event in
/// the [from, to] range. Empty location clears it. Back to the preview.
async fn calendar_bulk_location_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<BulkLocationForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    if f.cal_id.is_empty() || f.from.len() != 10 || f.to.len() != 10 || f.from > f.to {
        return Ok((StatusCode::BAD_REQUEST, "parâmetros inválidos").into_response());
    }
    let enc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    let after = format!("{}T00:00:00Z", f.from);
    let before = format!("{}T23:59:59Z", f.to);
    let loc = utf8_percent_encode(f.location.trim(), NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{enc}/events-by-range/set-location?after={after}&before={before}&location={loc}"
        ),
        &headers,
        Some((&t, &u)),
        &(),
    )
    .await?;
    let cenc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    Ok(Redirect::to(&format!(
        "/calendar/bulk-delete?cal_id={cenc}&from={}&to={}",
        f.from, f.to
    ))
    .into_response())
}

#[derive(Deserialize)]
struct BulkSummaryForm {
    cal_id: String,
    summary: String,
    from: String,
    to: String,
}

/// POST /calendar/bulk-summary — set the SUMMARY (title) of every event in the
/// [from, to] range. Title is required (backend rejects empty). Back to preview.
async fn calendar_bulk_summary_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<BulkSummaryForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let summary = f.summary.trim();
    if f.cal_id.is_empty()
        || summary.is_empty()
        || f.from.len() != 10
        || f.to.len() != 10
        || f.from > f.to
    {
        return Ok((StatusCode::BAD_REQUEST, "parâmetros inválidos").into_response());
    }
    let enc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    let after = format!("{}T00:00:00Z", f.from);
    let before = format!("{}T23:59:59Z", f.to);
    let summ = utf8_percent_encode(summary, NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.calendar,
        &format!(
            "/api/v1/calendars/{enc}/events-by-range/set-summary?after={after}&before={before}&summary={summ}"
        ),
        &headers,
        Some((&t, &u)),
        &(),
    )
    .await?;
    let cenc = utf8_percent_encode(&f.cal_id, NON_ALPHANUMERIC);
    Ok(Redirect::to(&format!(
        "/calendar/bulk-delete?cal_id={cenc}&from={}&to={}",
        f.from, f.to
    ))
    .into_response())
}

async fn freebusy_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<FreeBusyQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let attendees = q.attendees.unwrap_or_default();
    let date = q.date.unwrap_or_default();
    let emails: Vec<&str> = attendees
        .split([',', ';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let mut rows: Vec<FreeBusyRow> = Vec::new();
    let queried = !emails.is_empty() && date.len() == 10;
    if queried {
        let enc_att = utf8_percent_encode(&emails.join(","), NON_ALPHANUMERIC).to_string();
        let from = format!("{date}T00:00:00Z");
        let to = format!("{date}T23:59:59Z");
        let path = format!(
            "/api/v1/scheduling/freebusy?attendees={enc_att}&from={from}&to={to}&working_hours=true"
        );
        let resp = get_json::<serde_json::Value>(
            &st,
            &st.backends.calendar,
            &path,
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        let map = resp.get("attendees").and_then(|v| v.as_object());
        for email in &emails {
            let busy = map
                .and_then(|m| m.get(*email))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|iv| {
                            let s = iv.get("start")?.as_str()?;
                            let e = iv.get("end")?.as_str()?;
                            Some(format!("{}–{}", hhmm_of_rfc3339(s), hhmm_of_rfc3339(e)))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            rows.push(FreeBusyRow {
                email: (*email).to_string(),
                busy,
            });
        }
    }

    Ok(askama_axum::IntoResponse::into_response(FreeBusyTpl {
        me,
        attendees,
        date,
        rows,
        queried,
    }))
}

#[derive(Deserialize)]
struct ResourceConflictsQuery {
    from: String,
    to: String,
}

/// GET /calendar/resources/:id/conflicts?from=&to= — JSON proxy so the event
/// form can check a room's availability for the chosen window from the browser.
async fn resource_conflicts_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<ResourceConflictsQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok((StatusCode::UNAUTHORIZED, "não autenticado").into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let from = utf8_percent_encode(q.from.trim(), NON_ALPHANUMERIC);
    let to = utf8_percent_encode(q.to.trim(), NON_ALPHANUMERIC);
    let body = get_json::<serde_json::Value>(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/resources/{enc}/conflicts?from={from}&to={to}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| serde_json::json!({ "conflicts": [] }));
    Ok(json_response(&body))
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

#[derive(serde::Serialize)]
struct UndoSendPayload {
    from: String,
    to: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    cc: Vec<String>,
    subject: String,
    body_text: String,
    undo_seconds: i64,
}

/// POST /mail/send-undo — server-backed undo send. The backend holds the message
/// `undo_seconds` then relays it; returns `{id, deliver_at}` so the page shows a
/// countdown toast with a Cancel button. Real hold (survives tab close), unlike
/// the old client-only delay.
async fn mail_send_undo_action(
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
        return Ok((StatusCode::BAD_REQUEST, "Informe ao menos um destinatário.").into_response());
    }
    let undo_seconds = f
        .send_at
        .as_deref()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .unwrap_or(10)
        .clamp(5, 30);
    let (status, body) = crate::upstream::post_json_body(
        &st,
        &st.backends.mail,
        "/api/v1/mail/send-with-undo",
        &headers,
        Some((&t, &u)),
        &UndoSendPayload {
            from: f.from,
            to,
            cc: split_addrs(&f.cc),
            subject: f.subject,
            body_text: f.body_text,
            undo_seconds,
        },
    )
    .await?;
    if (200..300).contains(&status) {
        Ok(json_response(
            &body.unwrap_or_else(|| serde_json::json!({})),
        ))
    } else {
        Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
    }
}

/// POST /mail/:id/cancel-send — abort a held undo-send within its window.
async fn mail_cancel_send_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{enc}/cancel-send"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
}

// ─── /search (unified) ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct UnifiedSearchQuery {
    q: Option<String>,
    /// Category facet: a group label ("Mail"/"Drive"/…) to show alone, or
    /// empty/"all" for every group.
    #[serde(default)]
    r#type: Option<String>,
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

    let active_type = match uq.r#type.as_deref() {
        Some(t) if !t.is_empty() && !t.eq_ignore_ascii_case("all") => t.to_string(),
        _ => String::new(),
    };
    if qt.is_empty() {
        return Ok(askama_axum::IntoResponse::into_response(SearchTpl {
            me,
            query: String::new(),
            query_enc: String::new(),
            groups: Vec::new(),
            total: 0,
            facets: Vec::new(),
            active_type,
        }));
    }
    let all_groups = federate_search(&st, &headers, (t.as_str(), u.as_str()), qt).await;
    // Category facets reflect the full (unfiltered) result set so the counts
    // stay stable while the user narrows by type.
    let facets: Vec<SearchFacet> = all_groups
        .iter()
        .filter(|g| !g.hits.is_empty())
        .map(|g| SearchFacet {
            label: g.label.clone(),
            icon: g.icon.clone(),
            count: g.hits.len(),
        })
        .collect();
    let groups: Vec<SearchGroup> = if active_type.is_empty() {
        all_groups
    } else {
        all_groups
            .into_iter()
            .filter(|g| g.label == active_type)
            .collect()
    };
    let total: usize = groups.iter().map(|g| g.hits.len()).sum();

    let query_enc = utf8_percent_encode(&query, NON_ALPHANUMERIC).to_string();
    Ok(askama_axum::IntoResponse::into_response(SearchTpl {
        me,
        query,
        query_enc,
        groups,
        total,
        facets,
        active_type,
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

#[derive(Deserialize)]
struct ApplyPresetForm {
    preset_id: String,
    folder: Option<String>,
}

/// POST /mail/:id/apply-preset — add all flags from a saved preset to a message.
/// Resolves the preset's flags, then PATCHes the message flags ({add, remove:[]}).
async fn mail_apply_preset_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<ApplyPresetForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let folder = f.folder.unwrap_or_else(|| "INBOX".into());
    let back = format!(
        "/mail/{}?folder={}",
        utf8_percent_encode(&id, NON_ALPHANUMERIC),
        utf8_percent_encode(&folder, NON_ALPHANUMERIC)
    );
    // Resolve the preset's flags, then apply them in one PATCH.
    let penc = utf8_percent_encode(f.preset_id.trim(), NON_ALPHANUMERIC);
    let preset = get_json::<FlagPreset>(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/flag-presets/{penc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    if let Some(p) = preset {
        if !p.flags.is_empty() {
            let enc_id = utf8_percent_encode(&id, NON_ALPHANUMERIC);
            let _ = patch_json(
                &st,
                &st.backends.mail,
                &format!("/api/v1/mail/messages/{enc_id}/flags"),
                &headers,
                Some((&t, &u)),
                &serde_json::json!({ "add": p.flags, "remove": [] }),
            )
            .await;
        }
    }
    Ok(Redirect::to(&back).into_response())
}

#[derive(Deserialize)]
struct ReadReceiptForm {
    folder: Option<String>,
}

/// POST /mail/:id/read-receipt — send an MDN (read confirmation) to the original
/// sender, proxying the mail backend's read-receipt endpoint.
async fn mail_read_receipt_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<ReadReceiptForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = post_empty(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{id}/read-receipt"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    let folder = f.folder.unwrap_or_else(|| "INBOX".into());
    Ok(Redirect::to(&format!(
        "/mail/{}?folder={}",
        id,
        utf8_percent_encode(&folder, NON_ALPHANUMERIC)
    ))
    .into_response())
}

// ─── mail flow rules (automation) ─────────────────────────────────────────────

/// Summarize a rule's first condition into "campo op 'valor'" (the UI creates
/// single-condition rules; extra conditions from the API are noted as "+N").
fn summarize_conditions(conds: &serde_json::Value, mode: &str) -> String {
    let arr = conds.as_array().cloned().unwrap_or_default();
    if arr.is_empty() {
        return "qualquer mensagem".into();
    }
    let one = |c: &serde_json::Value| {
        let field = c.get("field").and_then(|v| v.as_str()).unwrap_or("?");
        let op = c.get("op").and_then(|v| v.as_str()).unwrap_or("contains");
        let value = c.get("value").and_then(|v| v.as_str()).unwrap_or("");
        format!("{field} {op} \"{value}\"")
    };
    let first = one(&arr[0]);
    if arr.len() == 1 {
        first
    } else {
        let joiner = if mode == "or" { "ou" } else { "e" };
        format!("{first} {joiner} +{}", arr.len() - 1)
    }
}

/// Summarize a rule's first action into readable text.
fn summarize_actions(actions: &serde_json::Value) -> String {
    let arr = actions.as_array().cloned().unwrap_or_default();
    let Some(a) = arr.first() else {
        return "nenhuma ação".into();
    };
    let kind = a.get("type").and_then(|v| v.as_str()).unwrap_or("?");
    let params = a.get("params");
    let p = |k: &str| {
        params
            .and_then(|v| v.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let base = match kind {
        "move_to_folder" => format!("mover para \"{}\"", p("folder")),
        "add_flag" => format!("marcar \"{}\"", p("flag")),
        "webhook" => format!("webhook {}", p("url")),
        other => other.to_string(),
    };
    if arr.len() > 1 {
        format!("{base} +{}", arr.len() - 1)
    } else {
        base
    }
}

// ── Export download proxies (notes JSON, mail folder mbox) ──

/// Proxy a backend download to the browser as an attachment, overriding the
/// content-type and filename. Returns 502 on upstream failure.
async fn download_proxy(
    st: &AppState,
    base: &str,
    path: &str,
    headers: &HeaderMap,
    ctx: (&str, &str),
    content_type: &'static str,
    filename: &str,
) -> WebResult<Response> {
    let (status, _ct, _cd, body) = get_bytes(st, base, path, headers, Some((ctx.0, ctx.1))).await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, "Falha ao exportar.").into_response());
    }
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}

/// GET /notes/export.json — download all of the caller's notes as a JSON backup.
async fn notes_export_json(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    download_proxy(
        &st,
        &st.backends.notes,
        "/api/v1/notes/export",
        &headers,
        (&t, &u),
        "application/json; charset=utf-8",
        "notes.json",
    )
    .await
}

/// GET /mail/folders/:id/export.mbox — download a mail folder as an mbox archive.
async fn mail_folder_export_mbox(
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
    download_proxy(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/folders/{enc}/export.mbox"),
        &headers,
        (&t, &u),
        "application/mbox",
        "folder.mbox",
    )
    .await
}

/// GET /flows — list the caller's mail automation rules.
async fn flows_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let raw = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.flows,
        "/api/v1/flows/rules",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let rules = raw
        .into_iter()
        .map(|r| {
            let mode = r
                .get("condition_mode")
                .and_then(|v| v.as_str())
                .unwrap_or("and");
            FlowRuleRow {
                id: r
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                name: r
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(sem nome)")
                    .to_string(),
                enabled: r.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                when: summarize_conditions(
                    r.get("conditions").unwrap_or(&serde_json::Value::Null),
                    mode,
                ),
                then: summarize_actions(r.get("actions").unwrap_or(&serde_json::Value::Null)),
            }
        })
        .collect();
    Ok(askama_axum::IntoResponse::into_response(FlowsTpl {
        me,
        rules,
    }))
}

#[derive(Deserialize)]
struct FlowCreateForm {
    name: String,
    field: String,
    op: String,
    value: String,
    action: String,
    action_value: String,
}

/// POST /flows — create a single-condition, single-action rule from the form.
async fn flow_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FlowCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let name = f.name.trim();
    let value = f.value.trim();
    if name.is_empty() || value.is_empty() {
        return Ok(Redirect::to("/flows").into_response());
    }
    // Map the action selector to the backend's {type, params} shape.
    let action = match f.action.as_str() {
        "add_flag" => {
            serde_json::json!({ "type": "add_flag", "params": { "flag": f.action_value.trim() } })
        }
        "webhook" => {
            serde_json::json!({ "type": "webhook", "params": { "url": f.action_value.trim() } })
        }
        // default: move to folder
        _ => {
            serde_json::json!({ "type": "move_to_folder", "params": { "folder": f.action_value.trim() } })
        }
    };
    let body = serde_json::json!({
        "name": name,
        "enabled": true,
        "conditions": [{ "field": f.field, "op": f.op, "value": value }],
        "condition_mode": "and",
        "actions": [action],
    });
    let _ = post_json(
        &st,
        &st.backends.flows,
        "/api/v1/flows/rules",
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await?;
    Ok(Redirect::to("/flows").into_response())
}

/// GET /flows/:id/edit — edit form pre-filled from the rule's first
/// condition/action (single-shape; complex rules flagged read-only-ish).
async fn flow_edit_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let Some(rule) = get_json::<serde_json::Value>(
        &st,
        &st.backends.flows,
        &format!("/api/v1/flows/rules/{id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    else {
        return Ok(Redirect::to("/flows").into_response());
    };
    let conds = rule
        .get("conditions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let acts = rule
        .get("actions")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let complex = conds.len() > 1 || acts.len() > 1;
    let c0 = conds.first();
    let a0 = acts.first();
    let str_of = |v: Option<&serde_json::Value>, k: &str| {
        v.and_then(|x| x.get(k))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string()
    };
    let action = a0
        .and_then(|x| x.get("type"))
        .and_then(|x| x.as_str())
        .unwrap_or("move_to_folder")
        .to_string();
    let action_value = match action.as_str() {
        "add_flag" => str_of(a0.and_then(|x| x.get("params")), "flag"),
        "webhook" => str_of(a0.and_then(|x| x.get("params")), "url"),
        _ => str_of(a0.and_then(|x| x.get("params")), "folder"),
    };
    Ok(askama_axum::IntoResponse::into_response(FlowEditTpl {
        me,
        id,
        name: rule
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        field: {
            let f = str_of(c0, "field");
            if f.is_empty() {
                "from".into()
            } else {
                f
            }
        },
        op: {
            let o = str_of(c0, "op");
            if o.is_empty() {
                "contains".into()
            } else {
                o
            }
        },
        value: str_of(c0, "value"),
        action,
        action_value,
        complex,
    }))
}

/// POST /flows/:id/edit — replace the rule's name/condition/action via PATCH.
async fn flow_edit_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<FlowCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let name = f.name.trim();
    let value = f.value.trim();
    if name.is_empty() || value.is_empty() {
        return Ok(Redirect::to(&format!("/flows/{id}/edit")).into_response());
    }
    let action = match f.action.as_str() {
        "add_flag" => {
            serde_json::json!({ "type": "add_flag", "params": { "flag": f.action_value.trim() } })
        }
        "webhook" => {
            serde_json::json!({ "type": "webhook", "params": { "url": f.action_value.trim() } })
        }
        _ => {
            serde_json::json!({ "type": "move_to_folder", "params": { "folder": f.action_value.trim() } })
        }
    };
    let body = serde_json::json!({
        "name": name,
        "conditions": [{ "field": f.field, "op": f.op, "value": value }],
        "condition_mode": "and",
        "actions": [action],
    });
    let _ = patch_json(
        &st,
        &st.backends.flows,
        &format!("/api/v1/flows/rules/{id}"),
        &headers,
        Some((&t, &u)),
        &body,
    )
    .await?;
    Ok(Redirect::to("/flows").into_response())
}

/// POST /flows/:id/toggle — enable/disable a rule.
async fn flow_toggle_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = patch_json(
        &st,
        &st.backends.flows,
        &format!("/api/v1/flows/rules/{id}/toggle"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({}),
    )
    .await?;
    Ok(Redirect::to("/flows").into_response())
}

#[derive(Deserialize)]
struct FlowReorderForm {
    /// Comma-separated rule ids in the new top-down (priority 1..N) order.
    ids: String,
}

/// POST /flows/reorder — assign priorities to rules by the submitted order.
/// The first id gets priority 1 (highest precedence). Proxies the flows
/// backend's bulk reorder (a `[{id, priority}]` array).
async fn flow_reorder_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FlowReorderForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let entries: Vec<serde_json::Value> = f
        .ids
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, id)| serde_json::json!({ "id": id, "priority": (i as i64) + 1 }))
        .collect();
    if entries.is_empty() {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let status = patch_json(
        &st,
        &st.backends.flows,
        "/api/v1/flows/rules/reorder",
        &headers,
        Some((&t, &u)),
        &serde_json::Value::Array(entries),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /flows/:id/delete — delete a rule.
async fn flow_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let _ = delete_at(
        &st,
        &st.backends.flows,
        &format!("/api/v1/flows/rules/{id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/flows").into_response())
}

// ─── /compliance (e-discovery archive) ───────────────────────────────────────

#[derive(Deserialize)]
struct ArchiveQuery {
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    from_addr: Option<String>,
    #[serde(default)]
    to_addr: Option<String>,
    /// CSV of tags; when present, tag search replaces the text filters.
    #[serde(default)]
    tags: Option<String>,
    /// "all" (AND, default) or "any" (OR).
    #[serde(default)]
    tag_mode: Option<String>,
    /// CSV of tags to exclude (only honoured with tag_mode "all").
    #[serde(default)]
    exclude: Option<String>,
}

/// Build the backend path for a tag search over the archive. `mode` "any"
/// uses the OR endpoint (union); anything else means AND (intersect),
/// switching to intersect-exclude when `exclude` has tags. Returns None when
/// `tags` has no usable entries (→ caller falls back to the text filters).
fn archive_tag_search_path(tags: &str, mode: &str, exclude: &str) -> Option<String> {
    fn enc_csv(raw: &str) -> String {
        raw.split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| utf8_percent_encode(t, NON_ALPHANUMERIC).to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
    let t = enc_csv(tags);
    if t.is_empty() {
        return None;
    }
    if mode == "any" {
        return Some(format!("/api/v1/compliance/archive/tags/union?tags={t}"));
    }
    let ex = enc_csv(exclude);
    if ex.is_empty() {
        Some(format!(
            "/api/v1/compliance/archive/tags/intersect?tags={t}"
        ))
    } else {
        Some(format!(
            "/api/v1/compliance/archive/tags/intersect-exclude?tags={t}&exclude={ex}"
        ))
    }
}

#[derive(Deserialize)]
struct ArchiveEntryJson {
    id: String,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    from_addr: Option<String>,
    #[serde(default)]
    to_addrs: serde_json::Value,
    #[serde(default)]
    archived_at: Option<String>,
    #[serde(default)]
    size_bytes: i64,
}

/// Compact byte size for the archive table (B/KB/MB).
fn archive_size(n: i64) -> String {
    let b = n as f64;
    if b < 1024.0 {
        format!("{n} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    }
}

/// Join a `to_addrs` JSON array into a comma list (first 3 + "…").
fn join_to_addrs(v: &serde_json::Value) -> String {
    let Some(arr) = v.as_array() else {
        return String::new();
    };
    let mut shown: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).take(3).collect();
    let label = shown.join(", ");
    if arr.len() > 3 {
        shown.clear();
        return format!("{label}, +{}", arr.len() - 3);
    }
    label
}

/// GET /compliance/archive — e-discovery search over the user's archived mail.
/// Filters by subject / from / to (ILIKE). Backend is participant-scoped.
async fn compliance_archive_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<ArchiveQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let subject = q.subject.unwrap_or_default();
    let from_addr = q.from_addr.unwrap_or_default();
    let to_addr = q.to_addr.unwrap_or_default();
    let tags = q.tags.unwrap_or_default();
    let tag_mode = match q.tag_mode.as_deref() {
        Some("any") => "any",
        _ => "all",
    }
    .to_string();
    let exclude = q.exclude.unwrap_or_default();
    let tag_path = archive_tag_search_path(&tags, &tag_mode, &exclude);
    let queried = tag_path.is_some()
        || !subject.trim().is_empty()
        || !from_addr.trim().is_empty()
        || !to_addr.trim().is_empty();

    let mut rows = Vec::new();
    if queried {
        // Tag search and text search are distinct backend endpoints; tags win.
        let path = if let Some(p) = tag_path {
            p
        } else {
            let mut qs: Vec<String> = vec!["limit=100".into()];
            for (k, val) in [
                ("subject", &subject),
                ("from_addr", &from_addr),
                ("to_addr", &to_addr),
            ] {
                let v = val.trim();
                if !v.is_empty() {
                    qs.push(format!("{k}={}", utf8_percent_encode(v, NON_ALPHANUMERIC)));
                }
            }
            format!("/api/v1/compliance/archive?{}", qs.join("&"))
        };
        let resp = get_json::<serde_json::Value>(
            &st,
            &st.backends.compliance,
            &path,
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default();
        if let Some(arr) = resp.get("entries").and_then(|v| v.as_array()) {
            for e in arr {
                if let Ok(a) = serde_json::from_value::<ArchiveEntryJson>(e.clone()) {
                    rows.push(ArchiveRow {
                        id: a.id,
                        subject: a.subject.unwrap_or_else(|| "(sem assunto)".into()),
                        from_addr: a.from_addr.unwrap_or_default(),
                        to_addrs: join_to_addrs(&a.to_addrs),
                        archived_at: a
                            .archived_at
                            .map(|s| s.replace('T', " ").chars().take(16).collect())
                            .unwrap_or_default(),
                        size_human: archive_size(a.size_bytes),
                    });
                }
            }
        }
    }
    Ok(askama_axum::IntoResponse::into_response(
        ComplianceArchiveTpl {
            me,
            subject,
            from_addr,
            to_addr,
            tags,
            tag_mode,
            exclude,
            rows,
            queried,
        },
    ))
}

/// GET /compliance/archive/export — download a ZIP of the archived messages
/// matching the current search filters (e-discovery export). Proxies the
/// compliance backend's ZIP export, streaming the bytes through.
async fn compliance_archive_export(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<ArchiveQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let mut qs: Vec<String> = Vec::new();
    for (k, val) in [
        ("subject", &q.subject),
        ("from_addr", &q.from_addr),
        ("to_addr", &q.to_addr),
    ] {
        if let Some(v) = val.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            qs.push(format!("{k}={}", utf8_percent_encode(v, NON_ALPHANUMERIC)));
        }
    }
    let query = if qs.is_empty() {
        String::new()
    } else {
        format!("?{}", qs.join("&"))
    };
    let (status, ct, _cd, body) = get_bytes(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/export{query}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, "Falha ao exportar.").into_response());
    }
    Ok((
        [
            (
                header::CONTENT_TYPE,
                ct.unwrap_or_else(|| "application/zip".into()),
            ),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"compliance-export.zip\"".to_string(),
            ),
        ],
        body,
    )
        .into_response())
}

#[derive(Deserialize)]
struct HoldForm {
    /// Comma-separated archive entry ids to (un)hold.
    ids: String,
    /// The hold tag (e.g. "hold-litigation"); defaults when blank.
    #[serde(default)]
    hold_tag: String,
}

/// Apply or remove a legal-hold tag on the selected archived messages via the
/// compliance bulk-hold/unhold endpoints. Returns the upstream status.
async fn compliance_hold(
    st: &AppState,
    headers: &HeaderMap,
    me: &Me,
    f: &HoldForm,
    hold: bool,
) -> WebResult<u16> {
    let (t, u) = ctx_of(me);
    let ids: Vec<&str> = f
        .ids
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let trimmed = f.hold_tag.trim();
    let tag = if trimmed.is_empty() {
        "hold-litigation"
    } else {
        trimmed
    };
    let path = if hold {
        "/api/v1/compliance/archive/bulk-hold"
    } else {
        "/api/v1/compliance/archive/bulk-unhold"
    };
    crate::upstream::post_json(
        st,
        &st.backends.compliance,
        path,
        headers,
        Some((&t, &u)),
        &serde_json::json!({ "archive_ids": ids, "hold_tag": tag }),
    )
    .await
}

/// POST /compliance/archive/hold — place selected messages under legal hold.
async fn compliance_hold_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<HoldForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if f.ids.trim().is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "selecione ao menos uma mensagem").into_response());
    }
    let status = compliance_hold(&st, &headers, &me, &f, true).await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /compliance/archive/unhold — release selected messages from legal hold.
async fn compliance_unhold_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<HoldForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if f.ids.trim().is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "selecione ao menos uma mensagem").into_response());
    }
    let status = compliance_hold(&st, &headers, &me, &f, false).await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

#[derive(Deserialize)]
struct ArchiveStatsQuery {
    #[serde(default)]
    since: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
}

/// Fetch a top-N archive list (`endpoint`) and map it to ranked rows. `key` is
/// the per-item label field ("sender"/"recipient"/"domain"); `wrap` is the
/// response array field name.
async fn archive_top_rows(
    st: &AppState,
    headers: &HeaderMap,
    ctx: (&str, &str),
    endpoint: &str,
    wrap: &str,
    key: &str,
    range: &str,
) -> WebResult<Vec<ArchiveStatRow>> {
    let resp = get_json::<serde_json::Value>(
        st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/{endpoint}?limit=10{range}"),
        headers,
        Some(ctx),
    )
    .await?
    .unwrap_or_default();
    let rows = resp
        .get(wrap)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|it| ArchiveStatRow {
                    label: it
                        .get(key)
                        .and_then(|v| v.as_str())
                        .unwrap_or("(?)")
                        .to_string(),
                    count: it
                        .get("count")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(rows)
}

/// GET /compliance/stats — e-discovery analytics: top senders, recipients and
/// domains over an optional date range.
async fn compliance_stats_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<ArchiveStatsQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let since = q.since.unwrap_or_default();
    let before = q.before.unwrap_or_default();
    let mut range = String::new();
    if since.len() == 10 {
        range.push_str(&format!("&since={since}"));
    }
    if before.len() == 10 {
        range.push_str(&format!("&before={before}"));
    }
    let ctx = (t.as_str(), u.as_str());
    let senders = archive_top_rows(
        &st,
        &headers,
        ctx,
        "top-senders",
        "senders",
        "sender",
        &range,
    )
    .await?;
    let recipients = archive_top_rows(
        &st,
        &headers,
        ctx,
        "top-recipients",
        "recipients",
        "recipient",
        &range,
    )
    .await?;
    let domains = archive_top_rows(
        &st,
        &headers,
        ctx,
        "top-domains",
        "domains",
        "domain",
        &range,
    )
    .await?;
    let subjects = archive_top_rows(
        &st,
        &headers,
        ctx,
        "top-subjects",
        "subjects",
        "subject",
        &range,
    )
    .await?;
    let bucket = match q.bucket.as_deref() {
        Some("week") => "week",
        Some("month") => "month",
        _ => "day",
    }
    .to_string();
    // Archiving volume over time (reuse the calendar histogram rendering).
    let hist = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/histogram?bucket={bucket}{range}"),
        &headers,
        Some(ctx),
    )
    .await?
    .unwrap_or_default();
    let points: Vec<HistogramPoint> = hist
        .get("series")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|p| serde_json::from_value(p.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    let max = points.iter().map(|p| p.count).max().unwrap_or(0).max(1);
    let volume: Vec<HistogramBar> = points
        .into_iter()
        .map(|p| HistogramBar {
            label: p
                .ts
                .as_deref()
                .map(|s| histogram_label(s, &bucket))
                .unwrap_or_default(),
            pct: ((p.count * 100) / max) as u32,
            count: p.count,
        })
        .collect();
    // Message-size distribution (7 fixed buckets, <1KB … >25MB).
    let size_resp = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        &format!(
            "/api/v1/compliance/archive/size-histogram?{}",
            range.trim_start_matches('&')
        ),
        &headers,
        Some(ctx),
    )
    .await?
    .unwrap_or_default();
    let size_counts: Vec<(String, i64)> = size_resp
        .get("buckets")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|b| {
                    (
                        b.get("bucket")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                        b.get("count")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let size_max = size_counts
        .iter()
        .map(|(_, c)| *c)
        .max()
        .unwrap_or(0)
        .max(1);
    let sizes: Vec<HistogramBar> = size_counts
        .into_iter()
        .map(|(label, count)| HistogramBar {
            label,
            pct: ((count * 100) / size_max) as u32,
            count,
        })
        .collect();
    Ok(askama_axum::IntoResponse::into_response(
        ComplianceStatsTpl {
            me,
            since,
            before,
            bucket,
            senders,
            recipients,
            domains,
            subjects,
            volume,
            sizes,
        },
    ))
}

/// GET /compliance/archive/:id/tags — the tags on one archived message as JSON.
async fn compliance_tags_get(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/{enc}/tags"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| serde_json::json!({ "tags": [] }));
    Ok(json_response(&v))
}

#[derive(Deserialize)]
struct ArchiveTagForm {
    tag: String,
}

/// POST /compliance/archive/:id/tags — add a tag to an archived message.
async fn compliance_tag_add(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<ArchiveTagForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tag = f.tag.trim();
    if tag.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "tag required").into_response());
    }
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = crate::upstream::post_json(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/{enc}/tags"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "tag": tag }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /compliance/archive/:id/tags/:tag/delete — remove a tag (proxies DELETE).
async fn compliance_tag_remove(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, tag)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let tenc = utf8_percent_encode(&tag, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/{enc}/tags/{tenc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// Map a tag rename/merge history response (`{entries: [...]}`) into table
/// rows. `from_key`/`to_key`/`count_key`/`at_key` name the per-entry fields
/// ("old_tag"/"new_tag"/"renamed_count"/"renamed_at" for renames,
/// "src_tag"/"dst_tag"/"merged_count"/"merged_at" for merges).
fn archive_tag_hist_rows(
    resp: &serde_json::Value,
    from_key: &str,
    to_key: &str,
    count_key: &str,
    at_key: &str,
) -> Vec<ArchiveTagHistRow> {
    resp.get("entries")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|e| ArchiveTagHistRow {
                    id: e
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    from_tag: e
                        .get(from_key)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    to_tag: e
                        .get(to_key)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    count: e
                        .get(count_key)
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    when: e
                        .get(at_key)
                        .and_then(|v| v.as_str())
                        .map(|s| s.replace('T', " ").chars().take(16).collect())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// GET /compliance/tags — manage the caller's archive tags: usage counts with
/// rename/merge actions, plus the rename/merge audit trails with undo.
async fn compliance_tags_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tags = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/archive/top-tags?limit=100",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .get("tags")
    .and_then(|v| v.as_array())
    .map(|arr| {
        arr.iter()
            .map(|it| ArchiveStatRow {
                label: it
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                count: it
                    .get("count")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })
            .collect()
    })
    .unwrap_or_default();
    let renames = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/archive/tags/rename-history?limit=20",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .map(|v| archive_tag_hist_rows(&v, "old_tag", "new_tag", "renamed_count", "renamed_at"))
    .unwrap_or_default();
    let merges = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/archive/tags/merge-history?limit=20",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .map(|v| archive_tag_hist_rows(&v, "src_tag", "dst_tag", "merged_count", "merged_at"))
    .unwrap_or_default();
    let pairs = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/archive/tags/co-occurrence?limit=50",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .get("pairs")
    .and_then(|v| v.as_array())
    .map(|arr| {
        arr.iter()
            .map(|p| TagPairRow {
                tag_a: p
                    .get("tag_a")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                tag_b: p
                    .get("tag_b")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                count: p
                    .get("co_count")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0),
            })
            .collect()
    })
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(
        ComplianceTagsTpl {
            me,
            tags,
            renames,
            merges,
            pairs,
        },
    ))
}

/// POST /compliance/tags/rename — rename a tag across the caller's archive.
async fn compliance_tag_rename_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<TagRenameForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tag = f.tag.trim();
    let new = f.new.trim();
    if tag.is_empty() || new.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "tag and new required").into_response());
    }
    let enc = utf8_percent_encode(tag, NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/tags/{enc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "new_tag": new }),
    )
    .await?;
    Ok(Redirect::to("/compliance/tags").into_response())
}

/// POST /compliance/tags/merge — merge a tag into another across the archive.
async fn compliance_tag_merge_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<TagMergeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tag = f.tag.trim();
    let into = f.into.trim();
    if tag.is_empty() || into.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "tag and into required").into_response());
    }
    let _ = post_json(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/archive/tags/merge",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "src": tag, "dst": into }),
    )
    .await?;
    Ok(Redirect::to("/compliance/tags").into_response())
}

/// POST /compliance/tags/rename-history/:id/undo — revert a past tag rename.
async fn compliance_tag_rename_undo(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = post_empty(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/tags/rename-history/{enc}/undo"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/compliance/tags").into_response())
}

/// POST /compliance/tags/merge-history/:id/undo — revert a past tag merge.
async fn compliance_tag_merge_undo(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = post_empty(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/archive/tags/merge-history/{enc}/undo"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/compliance/tags").into_response())
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

#[derive(Deserialize)]
struct SnoozeRecord {
    message_id: String,
    snooze_until: String,
}

/// GET /mail/snoozed — list messages snoozed by the current user, each with its
/// wake time, subject/sender (best-effort fetch), and open/wake-now actions.
/// One compose draft as the backend expects it (JSON, address arrays).
#[derive(Deserialize)]
struct DraftPayload {
    #[serde(default)]
    from: String,
    #[serde(default)]
    to: Vec<String>,
    #[serde(default)]
    cc: Vec<String>,
    #[serde(default)]
    bcc: Vec<String>,
    #[serde(default)]
    subject: String,
    #[serde(default)]
    body_text: String,
}

impl DraftPayload {
    fn to_backend(&self) -> serde_json::Value {
        serde_json::json!({
            "from": self.from,
            "to": self.to,
            "cc": self.cc,
            "bcc": self.bcc,
            "subject": self.subject,
            "body_text": self.body_text,
        })
    }
}

/// POST /mail/drafts — save a new compose draft (proxies JSON to the mail
/// service). The compose page's "💾 Rascunho" button used to POST FormData to
/// a non-existent route (404 → drafts never saved); this is the missing proxy.
async fn mail_draft_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    axum::Json(d): axum::Json<DraftPayload>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let (status, resp) = crate::upstream::post_json_body(
        &st,
        &st.backends.mail,
        "/api/v1/mail/drafts",
        &headers,
        Some((&t, &u)),
        &d.to_backend(),
    )
    .await?;
    match resp {
        Some(v) if (200..300).contains(&status) => Ok(json_response(&v)),
        _ => Ok(StatusCode::from_u16(status)
            .unwrap_or(StatusCode::BAD_GATEWAY)
            .into_response()),
    }
}

/// PUT /mail/drafts/:id — replace an existing draft (proxies JSON).
async fn mail_draft_update(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    axum::Json(d): axum::Json<DraftPayload>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let (status, resp) = put_json_body(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/drafts/{enc}"),
        &headers,
        Some((&t, &u)),
        &d.to_backend(),
    )
    .await?;
    match resp {
        Some(v) if (200..300).contains(&status) => Ok(json_response(&v)),
        _ => Ok(StatusCode::from_u16(status)
            .unwrap_or(StatusCode::BAD_GATEWAY)
            .into_response()),
    }
}

async fn mail_snoozed_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
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
    let records = get_json::<Vec<SnoozeRecord>>(
        &st,
        &st.backends.mail,
        "/api/v1/mail/snoozed",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();

    let mut rows = Vec::with_capacity(records.len());
    for rec in records {
        let enc = utf8_percent_encode(&rec.message_id, NON_ALPHANUMERIC);
        let detail = get_json::<MessageDetail>(
            &st,
            &st.backends.mail,
            &format!("/api/v1/mail/messages/{enc}"),
            &headers,
            Some((&t, &u)),
        )
        .await
        .ok()
        .flatten();
        let (subject, from) = detail
            .map(|d| {
                (
                    d.subject.unwrap_or_else(|| "(sem assunto)".into()),
                    d.from_name.or(d.from_addr).unwrap_or_default(),
                )
            })
            .unwrap_or_else(|| ("(mensagem indisponível)".into(), String::new()));
        rows.push(SnoozedRow {
            message_id: rec.message_id,
            wake_at: rec
                .snooze_until
                .replace('T', " ")
                .chars()
                .take(16)
                .collect(),
            subject,
            from,
        });
    }

    Ok(askama_axum::IntoResponse::into_response(MailSnoozedTpl {
        me,
        folders,
        rows,
    }))
}

/// POST /mail/:id/unsnooze — cancel a message's snooze (wake it now).
async fn mail_unsnooze_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/messages/{enc}/snooze"),
        &headers,
        Some((&t, &u)),
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

/// Parse the `{hits:[{file_id,name,snippet}]}` body from the drive content-search
/// endpoint into display rows. Missing fields default to empty.
fn drive_content_hits(body: &serde_json::Value) -> Vec<DriveContentHit> {
    let str_of = |h: &serde_json::Value, k: &str| {
        h.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    };
    body.get("hits")
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .map(|h| DriveContentHit {
                    file_id: str_of(h, "file_id"),
                    name: str_of(h, "name"),
                    snippet: str_of(h, "snippet"),
                })
                .filter(|h| !h.file_id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// GET /drive/content-search?q= — full-text search inside file *contents*
/// (the extracted-text index), distinct from the filename search on /drive.
async fn drive_content_search_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<DriveSearchQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let query = q.q.unwrap_or_default();
    let (hits, unavailable) = if query.trim().is_empty() {
        (vec![], false)
    } else {
        let enc = utf8_percent_encode(query.trim(), NON_ALPHANUMERIC);
        match get_json::<serde_json::Value>(
            &st,
            &st.backends.drive,
            &format!("/api/v1/drive/files/content-search?q={enc}&limit=50"),
            &headers,
            Some((&t, &u)),
        )
        .await
        {
            // Backend returns 503 when search isn't configured → flag, don't 500.
            Ok(Some(body)) => (drive_content_hits(&body), false),
            Ok(None) => (vec![], false),
            Err(_) => (vec![], true),
        }
    };
    Ok(askama_axum::IntoResponse::into_response(
        DriveContentSearchTpl {
            me,
            query,
            hits,
            unavailable,
        },
    ))
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

#[derive(Deserialize)]
struct GalSaveForm {
    email: String,
}

/// POST /contacts/gal/save — copy a directory (GAL) user into the caller's
/// personal addressbook. Idempotent on the backend (stable UID per directory
/// user). Returns the upstream status for the fetch-based UI.
async fn contacts_gal_save_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<GalSaveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let email = f.email.trim();
    if email.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "e-mail vazio").into_response());
    }
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.contacts,
        "/api/v1/gal/save",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "email": email }),
    )
    .await?;
    Ok((StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY)).into_response())
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

#[derive(Deserialize)]
struct DriveCommentJson {
    id: String,
    user_id: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    created_at: Option<String>,
}

/// GET /drive/:id/comments — file comment thread, authors resolved to emails.
async fn drive_comments_page(
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
    let file: Option<DriveFile> = get_json(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/metadata"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    let file_name = file.map(|f| f.name).unwrap_or_else(|| "Arquivo".into());
    let raw = get_json::<Vec<DriveCommentJson>>(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/comments"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let mut comments = Vec::with_capacity(raw.len());
    for c in raw {
        let mine = c.user_id == me.user_id;
        let author = resolve_email_by_id(&st, &c.user_id, &headers, &t, &u).await;
        comments.push(DriveCommentRow {
            id: c.id,
            author,
            body: c.body,
            when: c
                .created_at
                .map(|s| s.replace('T', " ").chars().take(16).collect())
                .unwrap_or_default(),
            mine,
        });
    }
    Ok(DriveCommentsTpl {
        me,
        file_id: id,
        file_name,
        comments,
    }
    .into_response())
}

#[derive(Deserialize)]
struct DriveCommentForm {
    body: String,
}

/// POST /drive/:id/comments — add a comment, then back to the thread.
async fn drive_comment_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<DriveCommentForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let body = f.body.trim();
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    if !body.is_empty() {
        let _ = post_json(
            &st,
            &st.backends.drive,
            &format!("/api/v1/drive/files/{enc}/comments"),
            &headers,
            Some((&t, &u)),
            &serde_json::json!({ "body": body }),
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/drive/{enc}/comments")).into_response())
}

/// POST /drive/:id/comments/:comment_id/delete — delete a comment (author only,
/// enforced by the backend).
async fn drive_comment_delete(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, comment_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let cenc = utf8_percent_encode(&comment_id, NON_ALPHANUMERIC).to_string();
    let _ = delete_at(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{enc}/comments/{cenc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/drive/{enc}/comments")).into_response())
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

/// GET /drive/:id/activity — change/audit history for a file.
async fn drive_activity_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let file_name = get_json::<DriveFile>(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/metadata"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .map(|f| f.name)
    .unwrap_or_default();
    let events = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.drive,
        &format!("/api/v1/drive/files/{id}/activity"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(activity_row_of)
    .collect();
    Ok(askama_axum::IntoResponse::into_response(DriveActivityTpl {
        me,
        file_id: id,
        file_name,
        events,
    }))
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
    let resources = fetch_resources(&st, &headers, &t, &u).await;
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
        categories: String::new(),
        resources,
        booked_resources: Vec::new(),
        error: None,
    }
    .into_response())
}

/// Fetch the tenant's bookable resources for the event form. Best-effort — an
/// error or missing list yields an empty catalog (the section just hides).
async fn fetch_resources(st: &AppState, headers: &HeaderMap, t: &str, u: &str) -> Vec<Resource> {
    let body = get_json::<serde_json::Value>(
        st,
        &st.backends.calendar,
        "/api/v1/resources",
        headers,
        Some((t, u)),
    )
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    body.get("resources")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

fn parse_attendees(raw: &str) -> Vec<String> {
    raw.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.contains('@'))
        .map(str::to_ascii_lowercase)
        .collect()
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
    let (status, created) = post_body_json(
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
    // Server-side reminder delivery reads calendar_event_alarms, which the iCal
    // VALARMs don't populate — enqueue them via the alarms API using the id of
    // the event we just created (best-effort; failures don't block the create).
    if let Some(event_id) = created
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
    {
        enqueue_reminders(&st, &headers, (&t, &u), &cal_id, event_id, &f.reminders).await;
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

/// Parse a comma-separated minutes-before list into sorted, deduped, capped
/// minutes (mirrors `ical::build_valarms`). Skips blanks/non-numbers.
fn reminder_minutes(reminders: &str) -> Vec<u32> {
    let mut mins: Vec<u32> = reminders
        .split(',')
        .filter_map(|s| s.trim().parse::<u32>().ok())
        .collect();
    mins.sort_unstable();
    mins.dedup();
    mins.truncate(10);
    mins
}

/// POST one DISPLAY alarm per reminder lead-time to the calendar alarms API so
/// the server-side delivery worker (which reads `calendar_event_alarms`, not the
/// stored iCal VALARMs) actually fires them. Best-effort: each failure is
/// ignored so a flaky alarm enqueue never fails the event create.
async fn enqueue_reminders(
    st: &AppState,
    headers: &HeaderMap,
    ctx: (&str, &str),
    cal_id: &str,
    event_id: &str,
    reminders: &str,
) {
    let cal_enc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC).to_string();
    let ev_enc = utf8_percent_encode(event_id, NON_ALPHANUMERIC).to_string();
    let path = format!("/api/v1/calendars/{cal_enc}/events/{ev_enc}/alarms");
    for m in reminder_minutes(reminders) {
        let body = serde_json::json!({
            "action": "DISPLAY",
            "trigger_rel": format!("-PT{m}M"),
            "description": "Lembrete",
        });
        let _ =
            crate::upstream::post_json(st, &st.backends.calendar, &path, headers, Some(ctx), &body)
                .await;
    }
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
        categories: event
            .ical_raw
            .as_deref()
            .map(categories_from_ical)
            .unwrap_or_default(),
        resources: fetch_resources(&st, &headers, &t, &u).await,
        booked_resources: event
            .ical_raw
            .as_deref()
            .map(booked_resources_from_ical)
            .unwrap_or_default(),
        error: None,
    }
    .into_response())
}

/// POST /calendar/:cal_id/events/:id/send-invite — re-send the event as an iTIP
/// REQUEST email to its human attendees (via the mail backend's send-itip). The
/// event's stored iCal is the invitation payload; the organizer (or the caller)
/// is the From. No-op with a clear status when the event has no attendees.
async fn event_send_invite_action(
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
    let Some(ics) = event.ical_raw.clone() else {
        return Ok((StatusCode::BAD_REQUEST, "Evento sem iCal armazenado.").into_response());
    };
    let to = crate::ical::attendee_emails_from_ical(&ics);
    if to.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            "O evento não tem participantes para convidar.",
        )
            .into_response());
    }
    let from = event
        .organizer_email
        .clone()
        .unwrap_or_else(|| me.email.clone());
    let subject = format!("Convite: {}", event.title());
    let status = crate::upstream::post_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/send-itip",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({
            "from": from,
            "to": to,
            "subject": subject,
            "method": "REQUEST",
            "ics": ics,
        }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
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
    // Re-sync server-side alarms to match the edited reminders: drop the event's
    // existing alarms, then re-enqueue from the form (best-effort).
    let _ = delete_at(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc_c}/events/{enc_e}/alarms"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    enqueue_reminders(&st, &headers, (&t, &u), &cal_id, &id, &f.reminders).await;
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
                categories: String::new(),
                resources: String::new(),
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
        emails: Vec::new(),
        addresses: Vec::new(),
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
    let emails = get_json::<Vec<ContactEmailRow>>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/emails"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let addresses = get_json::<Vec<ContactAddressRow>>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/addresses"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
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
        emails,
        addresses,
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

/// GET /contacts/:book_id/:id/photo — stream a contact's vCard PHOTO inline,
/// passing through the upstream content-type. 404 when the contact has no photo
/// (the `<img>` falls back via onerror). External URI photos are followed by the
/// HTTP client and streamed through.
async fn contact_photo(
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
    let (status, ct, _cd, body) = get_bytes(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/photo"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }
    let ct = ct.unwrap_or_else(|| "application/octet-stream".into());
    Ok((
        [
            (header::CONTENT_TYPE, ct),
            (header::CACHE_CONTROL, "private, max-age=86400".into()),
        ],
        body,
    )
        .into_response())
}

/// GET /contacts/:book_id/:id/activity — change history for a contact.
async fn contact_activity_page(
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
    let events = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/activity"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(activity_row_of)
    .collect();
    Ok(askama_axum::IntoResponse::into_response(
        ContactActivityTpl {
            me,
            book_id,
            contact_id: id,
            events,
        },
    ))
}

/// GET /contacts/:book_id/:id/versions — past vCard revisions of a contact.
async fn contact_versions_page(
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
    let versions = get_json::<Vec<ContactVersionRow>>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/versions"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    // The list is newest-first, so the first row's number is the diff target.
    let latest = versions.first().map(|v| v.version_no).unwrap_or(0);
    Ok(askama_axum::IntoResponse::into_response(
        ContactVersionsTpl {
            me,
            book_id,
            contact_id: id,
            versions,
            latest,
        },
    ))
}

/// POST /contacts/:book_id/:id/versions/:vno/restore — re-apply a past revision.
async fn contact_version_restore_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((book_id, id, vno)): Path<(String, String, i32)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = post_empty(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/versions/{vno}/restore"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/contacts/{enc_b}/{enc_i}/versions")).into_response())
}

/// GET /contacts/:book_id/:id/diff/:from/:to — line-level diff between two stored
/// vCard revisions of a contact (added/removed properties).
async fn contact_diff_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((book_id, id, from_no, to_no)): Path<(String, String, i32, i32)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc_b = utf8_percent_encode(&book_id, NON_ALPHANUMERIC).to_string();
    let enc_i = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let diff = get_json::<serde_json::Value>(
        &st,
        &st.backends.contacts,
        &format!("/api/v1/addressbooks/{enc_b}/contacts/{enc_i}/versions/{from_no}/diff/{to_no}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let lines = |key: &str| {
        diff.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(askama_axum::IntoResponse::into_response(ContactDiffTpl {
        me,
        book_id,
        contact_id: id,
        from_no,
        to_no,
        added: lines("added"),
        removed: lines("removed"),
    }))
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

/// POST /contacts/import-csv — forward the browser's multipart (book_id +
/// file) to the contacts CSV importer, which parses server-side (proper
/// quoting, validation, 4 MiB cap) — replaces the old fragile client-side
/// comma-split→vCard conversion. Returns the backend JSON ({imported: N}).
async fn contacts_import_csv(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("multipart/form-data")
        .to_string();
    let (status, resp) = crate::upstream::post_body_json(
        &st,
        &st.backends.contacts,
        "/api/v1/contacts/import",
        &headers,
        Some((&t, &u)),
        body,
        &ct,
    )
    .await?;
    if (200..300).contains(&status) {
        Ok(json_response(
            &resp.unwrap_or_else(|| serde_json::json!({ "imported": 0 })),
        ))
    } else {
        Ok(StatusCode::from_u16(status)
            .unwrap_or(StatusCode::BAD_GATEWAY)
            .into_response())
    }
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
    let (messages, attachments) = if let Some(ref ch) = active_channel {
        (
            chat_fetch_messages(&st, &headers, &t, &u, &ch.id, None).await,
            chat_fetch_attachments(&st, &headers, &t, &u, &ch.id).await,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(askama_axum::IntoResponse::into_response(ChatTpl {
        me,
        channels,
        active_channel,
        messages,
        attachments,
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
    let attachments = chat_fetch_attachments(&st, &headers, &t, &u, &cid).await;
    Ok(askama_axum::IntoResponse::into_response(ChatTpl {
        me,
        channels,
        active_channel,
        messages,
        attachments,
    }))
}

/// Fetch a channel's attachments for the "📎 Arquivos" panel. Best-effort —
/// errors yield an empty list (panel just shows nothing).
async fn chat_fetch_attachments(
    st: &AppState,
    headers: &HeaderMap,
    t: &str,
    u: &str,
    cid: &str,
) -> Vec<ChatAttachment> {
    let enc = utf8_percent_encode(cid, NON_ALPHANUMERIC);
    get_json::<Vec<ChatAttachment>>(
        st,
        &st.backends.chat,
        &format!("/api/v1/channels/{enc}/attachments?limit=200"),
        headers,
        Some((t, u)),
    )
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// GET /chat/channels/:cid/attachments/:aid/download — proxy a channel
/// attachment's bytes to the browser, passing through the backend's content-type
/// and content-disposition.
async fn chat_attachment_download(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cid, aid)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let cenc = utf8_percent_encode(&cid, NON_ALPHANUMERIC);
    let aenc = utf8_percent_encode(&aid, NON_ALPHANUMERIC);
    let (status, ct, cd, body) = get_bytes(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{cenc}/attachments/{aenc}/download"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok((StatusCode::BAD_GATEWAY, "Falha ao baixar.").into_response());
    }
    let ct = ct.unwrap_or_else(|| "application/octet-stream".into());
    let cd = cd.unwrap_or_else(|| "attachment".into());
    Ok((
        [
            (header::CONTENT_TYPE, ct),
            (header::CONTENT_DISPOSITION, cd),
        ],
        body,
    )
        .into_response())
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

/// POST /chat/channels/:cid/typing — broadcast a "typing…" event to the
/// channel (proxies the chat service; body-less). The compose JS already
/// called this path — there was no web route, so the indicator never fired.
async fn chat_typing(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&cid, NON_ALPHANUMERIC);
    let _ = post_empty(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{enc}/typing"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok((StatusCode::OK, "ok").into_response())
}

/// POST /chat/channels/:cid/presence/heartbeat — mark the caller present in
/// the channel (proxies the chat service; body-less).
async fn chat_presence_heartbeat(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&cid, NON_ALPHANUMERIC);
    let _ = post_empty(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{enc}/presence/heartbeat"),
        &headers,
        Some((&t, &u)),
    )
    .await;
    Ok((StatusCode::OK, "ok").into_response())
}

/// GET /chat/channels/:cid/presence — the channel's online roster as JSON.
async fn chat_presence_roster(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&cid, NON_ALPHANUMERIC);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{enc}/presence"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| serde_json::json!({ "online": [] }));
    Ok(json_response(&v))
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

// ── Chat message edit / delete ──

#[derive(serde::Deserialize)]
struct ChatEditForm {
    body: String,
}

/// PATCH /chat/channels/:cid/messages/:mid — edit a message (proxies the
/// backend's PUT, which the homeserver authorizes to the original sender).
async fn chat_edit_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cid, mid)): Path<(String, String)>,
    Form(f): Form<ChatEditForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let body = f.body.trim();
    if body.is_empty() {
        return Ok(StatusCode::BAD_REQUEST.into_response());
    }
    let url = format!(
        "{}/api/v1/channels/{cid}/messages/{mid}",
        st.backends.chat.trim_end_matches('/')
    );
    let mut req = st.http.put(&url).json(&serde_json::json!({ "body": body }));
    req = crate::upstream::fwd_cookie(req, &headers);
    req = crate::upstream::inject_ctx(req, &t, &u);
    let status = req.send().await.map(|r| r.status().as_u16()).unwrap_or(502);
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// DELETE /chat/channels/:cid/messages/:mid — redact a message (proxies the
/// backend's DELETE; the homeserver enforces redaction permission).
async fn chat_delete_message(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cid, mid)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = delete_at(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{cid}/messages/{mid}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// GET /chat/channels/:cid/messages/:mid/edits — the message's edit history
/// (Matrix `m.replace` relations) as JSON for the edit-history modal.
async fn chat_message_edits(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((cid, mid)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let edits = get_json::<serde_json::Value>(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{cid}/messages/{mid}/edits"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| serde_json::json!({ "chunk": [] }));
    Ok(json_response(&edits))
}

#[derive(serde::Deserialize)]
struct ChatInviteForm {
    email: String,
}

/// POST /chat/channels/:cid/members/invite — invite a tenant user (by email) to
/// the channel. The email is resolved to a user id via the contacts lookup,
/// then posted to the chat backend's add-member endpoint.
async fn chat_invite_member(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(cid): Path<String>,
    Form(f): Form<ChatInviteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let Some(user_id) =
        resolve_user_id(&st, &st.backends.contacts, f.email.trim(), &headers, &t, &u).await?
    else {
        return Ok((StatusCode::NOT_FOUND, "usuário não encontrado").into_response());
    };
    let status = post_json(
        &st,
        &st.backends.chat,
        &format!("/api/v1/channels/{cid}/members"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "user_id": user_id }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
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

/// POST /meet/:id/recordings/:rec_id/delete — remove a recording's metadata.
/// The meet backend gates this to moderator-or-creator.
async fn meet_recording_delete_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, rec_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let renc = utf8_percent_encode(&rec_id, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/recordings/{renc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /meet/:id/recording/start — mark server-side recording as started
/// (moderator-gated by the meet backend). Distinct from the Jitsi file
/// recording: this sets the meeting's `recording_started_at` for audit/status.
async fn meet_recording_start_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/recording/start"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /meet/:id/recording/stop — mark server-side recording as stopped
/// (moderator-gated by the meet backend).
async fn meet_recording_stop_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/recording/stop"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

// ── Meet polls (JSON proxies for the room page's poll panel) ──

fn json_response(v: &serde_json::Value) -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(v).unwrap_or_default(),
    )
        .into_response()
}

/// GET /meet/:id/polls — list a meeting's polls.
async fn meet_polls_list_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/polls"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!({ "polls": [] }));
    Ok(json_response(&v))
}

#[derive(Deserialize)]
struct PollCreateForm {
    question: String,
    /// Newline-separated option labels.
    options: String,
}

/// POST /meet/:id/polls — create a poll (moderator). Options come newline-split.
async fn meet_poll_create_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<PollCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let options: Vec<String> = f
        .options
        .split('\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if f.question.trim().is_empty() || options.len() < 2 {
        return Ok((StatusCode::BAD_REQUEST, "question + at least 2 options").into_response());
    }
    let status = post_json(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/polls"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "question": f.question.trim(), "options": options }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// GET /meet/:id/polls/:poll_id — poll detail with tallies + my_vote.
async fn meet_poll_get_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, poll_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/polls/{poll_id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!({}));
    Ok(json_response(&v))
}

#[derive(Deserialize)]
struct PollVoteForm {
    option_idx: i32,
}

/// POST /meet/:id/polls/:poll_id/vote — cast a vote.
async fn meet_poll_vote_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, poll_id)): Path<(String, String)>,
    Form(f): Form<PollVoteForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/polls/{poll_id}/vote"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "option_idx": f.option_idx }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

// ── Meet lobby (waiting room) — moderator approval ──

/// GET /meet/:id/lobby — waiting users, each resolved to an email for display.
async fn meet_lobby_list_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/lobby"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!({ "waiting": [] }));
    // Resolve user ids to emails so the moderator sees who's waiting.
    let mut waiting = Vec::new();
    if let Some(arr) = v.get("waiting").and_then(|w| w.as_array()) {
        for entry in arr {
            if let Some(uid) = entry.get("user_id").and_then(|x| x.as_str()) {
                let email = resolve_email_by_id(&st, uid, &headers, &t, &u).await;
                waiting.push(serde_json::json!({ "user_id": uid, "email": email }));
            }
        }
    }
    Ok(json_response(&serde_json::json!({ "waiting": waiting })))
}

/// POST /meet/:id/lobby/:user_id/approve — admit a waiting user (moderator).
async fn meet_lobby_approve_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, user_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = post_empty(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/lobby/approve/{user_id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /meet/:id/lobby/:user_id/deny — remove a waiting user (moderator).
async fn meet_lobby_deny_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, user_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = delete_at(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{id}/lobby/{user_id}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// GET /meet/:id/transcripts — list a meeting's stored transcripts (JSON for the
/// meet-room transcripts panel). Participant-gated by the meet backend.
async fn meet_transcripts_list_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/transcript"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!([]));
    Ok(json_response(&v))
}

#[derive(Deserialize)]
struct TranscriptSearchQuery {
    #[serde(default)]
    q: String,
}

/// GET /meet/:id/transcripts/search?q= — fulltext search over a meeting's
/// transcripts (proxies the meet backend, participant-gated). JSON for the
/// transcripts panel search box.
async fn meet_transcripts_search_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Query(qs): Query<TranscriptSearchQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let qenc = utf8_percent_encode(qs.q.trim(), NON_ALPHANUMERIC);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/transcript/search?q={qenc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!([]));
    Ok(json_response(&v))
}

/// GET /meet/:id/breakouts — list breakout rooms (+ participants) as JSON.
async fn meet_breakouts_list_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/breakouts"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or(serde_json::json!([]));
    // Resolve each participant id to an email so the moderator sees who is in
    // each room (the backend stores only user ids).
    let mut rooms = Vec::new();
    if let Some(arr) = v.as_array() {
        for room in arr {
            let mut members = Vec::new();
            if let Some(ps) = room.get("participants").and_then(|p| p.as_array()) {
                for p in ps {
                    if let Some(uid) = p.as_str() {
                        let email = resolve_email_by_id(&st, uid, &headers, &t, &u).await;
                        members.push(serde_json::json!({ "user_id": uid, "email": email }));
                    }
                }
            }
            let mut obj = room.clone();
            if let Some(map) = obj.as_object_mut() {
                map.insert("participants".into(), serde_json::Value::Array(members));
            }
            rooms.push(obj);
        }
    }
    Ok(json_response(&serde_json::Value::Array(rooms)))
}

#[derive(Deserialize)]
struct BreakoutCreateForm {
    name: String,
}

/// POST /meet/:id/breakouts — create a breakout room (moderator).
async fn meet_breakout_create_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<BreakoutCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let name = f.name.trim();
    if name.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "name required").into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_json(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/breakouts"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "name": name }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /meet/:id/breakouts/:room_id/delete — delete a breakout room (moderator).
async fn meet_breakout_delete_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, room_id)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let renc = utf8_percent_encode(&room_id, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/breakouts/{renc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

#[derive(Deserialize)]
struct BreakoutAssignForm {
    email: String,
}

#[derive(Deserialize)]
struct BreakoutRemoveForm {
    user_id: String,
}

/// POST /meet/:id/breakouts/:room_id/participants — assign a participant to a
/// breakout room (moderator). Resolves the email to a user id for the backend.
async fn meet_breakout_assign_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, room_id)): Path<(String, String)>,
    Form(f): Form<BreakoutAssignForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let email = f.email.trim();
    if email.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "email required").into_response());
    }
    let Some(user_id) =
        resolve_user_id(&st, &st.backends.contacts, email, &headers, &t, &u).await?
    else {
        return Ok((StatusCode::NOT_FOUND, "user not found").into_response());
    };
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let renc = utf8_percent_encode(&room_id, NON_ALPHANUMERIC);
    let status = post_json(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/breakouts/{renc}/participants"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "user_id": user_id }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// DELETE /meet/:id/breakouts/:room_id/participants — remove a participant from
/// a breakout room (moderator). The backend takes the user id in the body.
async fn meet_breakout_remove_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, room_id)): Path<(String, String)>,
    Form(f): Form<BreakoutRemoveForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let user_id = f.user_id.trim();
    if user_id.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "user_id required").into_response());
    }
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let renc = utf8_percent_encode(&room_id, NON_ALPHANUMERIC);
    let status = delete_json(
        &st,
        &st.backends.meet,
        &format!("/api/v1/meetings/{enc}/breakouts/{renc}/participants"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "user_id": user_id }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
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

/// Resolve the user's default calendar id (falls back to the first one). Empty
/// when the user has no calendars.
async fn default_calendar_id(st: &AppState, headers: &HeaderMap, t: &str, u: &str) -> String {
    let cals = get_json::<Vec<Calendar>>(
        st,
        &st.backends.calendar,
        "/api/v1/calendars",
        headers,
        Some((t, u)),
    )
    .await
    .ok()
    .flatten()
    .unwrap_or_default();
    cals.iter()
        .find(|c| c.is_default)
        .or_else(|| cals.first())
        .map(|c| c.id.clone())
        .unwrap_or_default()
}

async fn tasks_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let cal_id = default_calendar_id(&st, &headers, &t, &u).await;
    let tasks = if cal_id.is_empty() {
        Vec::new()
    } else {
        let enc = utf8_percent_encode(&cal_id, NON_ALPHANUMERIC);
        get_json::<Vec<TaskRow>>(
            &st,
            &st.backends.calendar,
            &format!("/api/v1/calendars/{enc}/tasks"),
            &headers,
            Some((&t, &u)),
        )
        .await?
        .unwrap_or_default()
    };
    Ok(askama_axum::IntoResponse::into_response(TasksTpl {
        me,
        tasks,
        cal_id,
    }))
}

#[derive(Deserialize)]
struct TaskCreateForm {
    summary: String,
    #[serde(default)]
    due: String,
    #[serde(default)]
    priority: String,
    /// Recurrence: "" (one-off) / "daily" / "weekly" / "monthly".
    #[serde(default)]
    repeat: String,
    cal_id: String,
}

/// POST /tasks/create — create a server-backed VTODO in the user's calendar.
async fn tasks_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<TaskCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (summary, cal_id) = (f.summary.trim(), f.cal_id.trim());
    if summary.is_empty() || cal_id.is_empty() {
        return Ok(Redirect::to("/tasks").into_response());
    }
    let mut payload = serde_json::json!({ "summary": summary });
    // The due input is a date ("YYYY-MM-DD"); make it midnight RFC3339.
    let due = f.due.trim();
    if due.len() == 10 {
        payload["due"] = serde_json::json!(format!("{due}T00:00:00Z"));
    }
    if let Ok(p) = f.priority.trim().parse::<i16>() {
        if (1..=9).contains(&p) {
            payload["priority"] = serde_json::json!(p);
        }
    }
    // Recurrence (RFC 5545, expanded server-side via the instances endpoint).
    let rrule = match f.repeat.as_str() {
        "daily" => Some("FREQ=DAILY"),
        "weekly" => Some("FREQ=WEEKLY"),
        "monthly" => Some("FREQ=MONTHLY"),
        _ => None,
    };
    if let Some(r) = rrule {
        payload["rrule"] = serde_json::json!(r);
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC);
    let _ = post_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{enc}/tasks"),
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await?;
    Ok(Redirect::to("/tasks").into_response())
}

#[derive(Deserialize)]
struct TaskActionForm {
    cal_id: String,
    /// For complete: "1" marks COMPLETED, "0" reopens to NEEDS-ACTION.
    #[serde(default)]
    done: String,
}

/// POST /tasks/:id/complete — toggle a task's COMPLETED status.
async fn tasks_complete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<TaskActionForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let cal_id = f.cal_id.trim();
    if cal_id.is_empty() {
        return Ok(Redirect::to("/tasks").into_response());
    }
    let status = if f.done == "1" {
        "COMPLETED"
    } else {
        "NEEDS-ACTION"
    };
    let (t, u) = ctx_of(&me);
    let cenc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC);
    let ienc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{cenc}/tasks/{ienc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "status": status }),
    )
    .await?;
    Ok(Redirect::to("/tasks").into_response())
}

#[derive(Deserialize)]
struct TaskRepeatForm {
    cal_id: String,
    /// "" (one-off / clear) / "daily" / "weekly" / "monthly".
    #[serde(default)]
    repeat: String,
}

/// POST /tasks/:id/repeat — change a task's recurrence (PATCH rrule; ""
/// clears it back to one-off via a JSON null, the backend's doubly-optional).
async fn tasks_repeat_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<TaskRepeatForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let cal_id = f.cal_id.trim();
    if cal_id.is_empty() {
        return Ok(Redirect::to("/tasks").into_response());
    }
    let rrule = match f.repeat.as_str() {
        "daily" => serde_json::json!("FREQ=DAILY"),
        "weekly" => serde_json::json!("FREQ=WEEKLY"),
        "monthly" => serde_json::json!("FREQ=MONTHLY"),
        _ => serde_json::Value::Null,
    };
    let (t, u) = ctx_of(&me);
    let cenc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC);
    let ienc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{cenc}/tasks/{ienc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "rrule": rrule }),
    )
    .await?;
    Ok(Redirect::to("/tasks").into_response())
}

/// POST /tasks/:id/delete — delete a task.
async fn tasks_delete_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<TaskActionForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let cal_id = f.cal_id.trim();
    if cal_id.is_empty() {
        return Ok(Redirect::to("/tasks").into_response());
    }
    let (t, u) = ctx_of(&me);
    let cenc = utf8_percent_encode(cal_id, NON_ALPHANUMERIC);
    let ienc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = delete_at(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/calendars/{cenc}/tasks/{ienc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/tasks").into_response())
}

// ─── /notes ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct NotesQuery {
    id: Option<String>,
    /// Notebook filter: a notebook id, the literal "none" (loose notes), or
    /// absent (all notes).
    notebook: Option<String>,
}

#[derive(Deserialize)]
struct NoteForm {
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    /// Selected notebook id; empty string means "no notebook".
    #[serde(default)]
    notebook_id: String,
}

#[derive(Deserialize)]
struct NotebookForm {
    #[serde(default)]
    name: String,
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
    let current_notebook = q.notebook.clone().unwrap_or_default();
    let notes_path = if current_notebook.is_empty() {
        "/api/v1/notes".to_string()
    } else {
        let enc = utf8_percent_encode(&current_notebook, NON_ALPHANUMERIC);
        format!("/api/v1/notes?notebook={enc}")
    };
    let notes = get_json::<Vec<Note>>(
        &st,
        &st.backends.notes,
        &notes_path,
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let notebooks = get_json::<Vec<Notebook>>(
        &st,
        &st.backends.notes,
        "/api/v1/notebooks",
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
                notebook_id: n.notebook_id.clone(),
            });
    Ok(askama_axum::IntoResponse::into_response(NotesTpl {
        me,
        notes,
        selected,
        notebooks,
        current_notebook,
    }))
}

/// GET /notes/:id/activity — change history for a note.
async fn notes_activity_page(
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
    let events = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}/activity"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(activity_row_of)
    .collect();
    Ok(askama_axum::IntoResponse::into_response(NotesActivityTpl {
        me,
        note_id: id,
        events,
    }))
}

/// GET /notes/:id/tags — current tags of a note as a JSON array (for the editor
/// tags field).
async fn notes_tags_get_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let tags = get_json::<serde_json::Value>(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}/tags"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| serde_json::json!([]));
    Ok(json_response(&tags))
}

#[derive(Deserialize)]
struct NoteTagsForm {
    #[serde(default)]
    tags: String,
}

/// POST /notes/:id/tags — replace a note's tags (CSV/space-separated input).
/// Returns the resulting tag array so the editor field can refresh.
async fn notes_tags_set_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<NoteTagsForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tags: Vec<String> = f
        .tags
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = put_json(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}/tags"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "tags": tags }),
    )
    .await?;
    if !(200..300).contains(&status) {
        return Ok(StatusCode::from_u16(status)
            .unwrap_or(StatusCode::BAD_GATEWAY)
            .into_response());
    }
    Ok(json_response(&serde_json::json!(tags)))
}

/// GET /notes/shared — notes other users have shared with the caller.
async fn notes_shared_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let rows = get_json::<Vec<SharedNoteRow>>(
        &st,
        &st.backends.notes,
        "/api/v1/notes/shared",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(NotesSharedTpl {
        me,
        rows,
    }))
}

/// GET /notes/:id/versions — past content revisions of a note (newest first).
async fn notes_versions_page(
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
    let versions = get_json::<Vec<NoteVersionRow>>(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}/versions"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(NoteVersionsTpl {
        me,
        note_id: id,
        versions,
    }))
}

/// POST /notes/:id/versions/:vno/restore — restore a prior version's content.
/// Reversible: the backend snapshots the current content first.
async fn notes_version_restore_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, vno)): Path<(String, i32)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = post_empty(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/{enc}/versions/{vno}/restore"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to(&format!("/notes/{enc}/versions")).into_response())
}

/// Map a backend activity JSON event to a display row (action/detail/when).
/// `detail` may be a plain string or a JSON object (drive uses JSON) — render a
/// string either way.
fn activity_row_of(e: serde_json::Value) -> ActivityRow {
    let detail = match e.get("detail") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(v) => serde_json::to_string(v).unwrap_or_default(),
    };
    ActivityRow {
        action: e
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        detail,
        when: e
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    }
}

/// GET /notes/tags — tag relationships: pairs of tags that appear together on
/// the caller's notes, most-co-occurring first.
async fn notes_tags_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let pairs = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.notes,
        "/api/v1/notes/tags/co-occurrence?limit=100",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(|p| TagPairRow {
        tag_a: p
            .get("tag_a")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        tag_b: p
            .get("tag_b")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        count: p
            .get("count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    })
    .collect();
    let stats = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.notes,
        "/api/v1/notes/tags/stats",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(|s| NoteTagStat {
        tag: s
            .get("tag")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        count: s
            .get("count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
    })
    .collect();
    Ok(askama_axum::IntoResponse::into_response(NotesTagsTpl {
        me,
        pairs,
        stats,
    }))
}

#[derive(Deserialize)]
struct TagRenameForm {
    tag: String,
    new: String,
}

/// POST /notes/tags/rename — rename a tag across all the caller's notes.
async fn notes_tag_rename_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<TagRenameForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tag = f.tag.trim();
    let new = f.new.trim();
    if tag.is_empty() || new.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "tag and new required").into_response());
    }
    let enc = utf8_percent_encode(tag, NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/tags/{enc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "new": new }),
    )
    .await?;
    Ok(Redirect::to("/notes/tags").into_response())
}

#[derive(Deserialize)]
struct TagMergeForm {
    tag: String,
    into: String,
}

/// POST /notes/tags/merge — merge a tag into another across the caller's notes.
async fn notes_tag_merge_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<TagMergeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let tag = f.tag.trim();
    let into = f.into.trim();
    if tag.is_empty() || into.is_empty() {
        return Ok((StatusCode::BAD_REQUEST, "tag and into required").into_response());
    }
    let enc = utf8_percent_encode(tag, NON_ALPHANUMERIC);
    let _ = post_json(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notes/tags/{enc}/merge"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "into": into }),
    )
    .await?;
    Ok(Redirect::to("/notes/tags").into_response())
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
    let mut body = serde_json::json!({ "title": f.title, "body": f.body });
    // Assign to a notebook only when one was picked (empty = loose note → omit).
    if !f.notebook_id.is_empty() {
        body["notebook_id"] = serde_json::json!(f.notebook_id);
    }
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
    // notebook_id is always sent on edit: a chosen id assigns/moves, empty
    // detaches (null). UpdateNote.notebook_id is Option<Option<Uuid>>.
    let notebook = if f.notebook_id.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(f.notebook_id)
    };
    let body = serde_json::json!({ "title": f.title, "body": f.body, "notebook_id": notebook });
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

/// POST /notes/notebooks — create a notebook, then show its notes.
async fn notes_notebook_create_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<NotebookForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let name = f.name.trim();
    if name.is_empty() {
        return Ok(Redirect::to("/notes").into_response());
    }
    let (t, u) = ctx_of(&me);
    let _ = post_json(
        &st,
        &st.backends.notes,
        "/api/v1/notebooks",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "name": name }),
    )
    .await?;
    Ok(Redirect::to("/notes").into_response())
}

/// POST /notes/notebooks/:id/rename — rename a notebook.
async fn notes_notebook_rename_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<NotebookForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let name = f.name.trim();
    if name.is_empty() {
        return Ok(Redirect::to(&format!("/notes?notebook={id}")).into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC).to_string();
    let _ = patch_json(
        &st,
        &st.backends.notes,
        &format!("/api/v1/notebooks/{enc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "name": name }),
    )
    .await?;
    Ok(Redirect::to(&format!("/notes?notebook={enc}")).into_response())
}

/// POST /notes/notebooks/:id/delete — delete a notebook (its notes detach, not
/// deleted), then back to all notes.
async fn notes_notebook_delete_action(
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
        &format!("/api/v1/notebooks/{enc}"),
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
struct NotificationsForm {
    #[serde(default)]
    notify_new_mail: Option<String>,
    #[serde(default)]
    notify_flags_changed: Option<String>,
    #[serde(default)]
    notify_folder_updated: Option<String>,
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

    // Notification preferences (per-kind; absent kind = enabled by default).
    let (mut notify_new_mail, mut notify_flags_changed, mut notify_folder_updated) =
        (true, true, true);
    if tab == "notifications" {
        if let Some(v) = get_json::<serde_json::Value>(
            &st,
            &st.backends.notifications,
            "/api/v1/notifications/preferences",
            &headers,
            Some((&t, &u)),
        )
        .await?
        {
            if let Some(arr) = v.get("preferences").and_then(|p| p.as_array()) {
                for row in arr {
                    let kind = row.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                    let enabled = row.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true);
                    match kind {
                        "new_mail" => notify_new_mail = enabled,
                        "flags_changed" => notify_flags_changed = enabled,
                        "folder_updated" => notify_folder_updated = enabled,
                        _ => {}
                    }
                }
            }
        }
    }

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

    // Load flag presets only on that tab.
    let flag_presets = if tab == "flag_presets" {
        get_json::<Vec<FlagPreset>>(
            &st,
            &st.backends.mail,
            "/api/v1/mail/flag-presets",
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
        flag_presets,
        working_days,
        notify_new_mail,
        notify_flags_changed,
        notify_folder_updated,
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
    Form(f): Form<NotificationsForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    // A checkbox present in the form = enabled; absent = disabled. PUT each kind
    // (the backend stores one row per kind; absent row = enabled by default).
    let kinds = [
        ("new_mail", f.notify_new_mail.is_some()),
        ("flags_changed", f.notify_flags_changed.is_some()),
        ("folder_updated", f.notify_folder_updated.is_some()),
    ];
    for (kind, enabled) in kinds {
        let _ = put_json(
            &st,
            &st.backends.notifications,
            "/api/v1/notifications/preferences",
            &headers,
            Some((&t, &u)),
            &serde_json::json!({ "kind": kind, "enabled": enabled }),
        )
        .await;
    }
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

#[derive(Deserialize)]
struct SieveTestForm {
    script: String,
    raw_message: String,
}

/// POST /settings/filters/test — evaluate a Sieve script against a sample
/// message without saving (proxies the mail sieve/test endpoint). Returns the
/// backend JSON ({actions:[…]}) on success, or the upstream status+body so the
/// JS can surface a syntax/validation error.
async fn settings_filters_test(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    axum::Json(f): axum::Json<SieveTestForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let (status, resp) = crate::upstream::post_json_body(
        &st,
        &st.backends.mail,
        "/api/v1/mail/sieve/test",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "script": f.script, "raw_message": f.raw_message }),
    )
    .await?;
    match resp {
        Some(v) if (200..300).contains(&status) => Ok(json_response(&v)),
        _ => Ok((
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            "Script inválido ou serviço indisponível.",
        )
            .into_response()),
    }
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

// ─── flag presets ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct FlagPresetForm {
    name: String,
    /// Comma/space-separated IMAP flags (e.g. "\\Flagged, Urgente").
    #[serde(default)]
    flags: String,
}

/// POST /settings/flag-presets — create a named set of IMAP flags.
async fn settings_flag_preset_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<FlagPresetForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let name = f.name.trim();
    let flags: Vec<String> = f
        .flags
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if name.is_empty() || flags.is_empty() {
        return Ok(Redirect::to(
            "/settings?tab=flag_presets&flash=Informe+nome+e+ao+menos+uma+flag",
        )
        .into_response());
    }
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.mail,
        "/api/v1/mail/flag-presets",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "name": name, "flags": flags }),
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Preset+criado"
    } else {
        "Falha+ao+criar+preset"
    };
    Ok(Redirect::to(&format!("/settings?tab=flag_presets&flash={flash}")).into_response())
}

/// POST /settings/flag-presets/:id/delete — remove a flag preset.
async fn settings_flag_preset_delete(
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
        &format!("/api/v1/mail/flag-presets/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/settings?tab=flag_presets&flash=Preset+removido").into_response())
}

/// POST /settings/flag-presets/:id/edit — update a preset's name and flags
/// (PUT upstream; same CSV parsing as create).
async fn settings_flag_preset_edit(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<FlagPresetForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let name = f.name.trim();
    let flags: Vec<String> = f
        .flags
        .split([',', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if name.is_empty() || flags.is_empty() {
        return Ok(Redirect::to(
            "/settings?tab=flag_presets&flash=Informe+nome+e+ao+menos+uma+flag",
        )
        .into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = put_json(
        &st,
        &st.backends.mail,
        &format!("/api/v1/mail/flag-presets/{enc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "name": name, "flags": flags }),
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Preset+atualizado"
    } else {
        "Falha+ao+atualizar+preset"
    };
    Ok(Redirect::to(&format!("/settings?tab=flag_presets&flash={flash}")).into_response())
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

// ─── /settings/tokens (personal access tokens) ───────────────────────────────

/// Map one backend token-info JSON into a table row. Backend list is newest-
/// first; timestamps come as RFC 3339.
fn api_token_row(v: &serde_json::Value) -> ApiTokenRow {
    let short = |key: &str| -> String {
        v.get(key)
            .and_then(|x| x.as_str())
            .map(|s| s.replace('T', " ").chars().take(16).collect())
            .unwrap_or_default()
    };
    ApiTokenRow {
        id: v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        name: v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        created: short("created_at"),
        last_used: short("last_used_at"),
        expires: short("expires_at"),
        active: v
            .get("revoked_at")
            .map(serde_json::Value::is_null)
            .unwrap_or(true),
    }
}

/// Fetch the caller's tokens (bare array) as table rows.
async fn fetch_api_tokens(st: &AppState, headers: &HeaderMap) -> WebResult<Vec<ApiTokenRow>> {
    Ok(
        get_json::<Vec<serde_json::Value>>(st, &st.backends.auth, "/auth/tokens", headers, None)
            .await?
            .unwrap_or_default()
            .iter()
            .map(api_token_row)
            .collect(),
    )
}

/// GET /settings/tokens — list the caller's personal access tokens.
async fn settings_tokens_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let rows = fetch_api_tokens(&st, &headers).await?;
    Ok(askama_axum::IntoResponse::into_response(
        SettingsTokensTpl {
            me,
            rows,
            new_token: None,
            flash: extract_flash(&uri),
        },
    ))
}

#[derive(Deserialize)]
struct TokenCreateForm {
    name: String,
    /// Lifetime in days as a string; empty or "never" = non-expiring.
    #[serde(default)]
    expires_days: String,
}

/// POST /settings/tokens — mint a token and render the page with the
/// cleartext shown exactly once (no redirect, or the secret would be lost).
async fn settings_token_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<TokenCreateForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let name = f.name.trim();
    if name.is_empty() || name.len() > 128 {
        return Ok((StatusCode::BAD_REQUEST, "nome inválido").into_response());
    }
    let expires_in_seconds = f
        .expires_days
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|d| (1..=365).contains(d))
        .map(|d| d * 86400);
    let mut body = serde_json::json!({ "name": name });
    if let Some(s) = expires_in_seconds {
        body["expires_in_seconds"] = serde_json::json!(s);
    }
    let (status, resp) = crate::upstream::post_json_body(
        &st,
        &st.backends.auth,
        "/auth/tokens",
        &headers,
        None,
        &body,
    )
    .await?;
    let new_token = resp
        .as_ref()
        .and_then(|v| v.get("token"))
        .and_then(|v| v.as_str())
        .map(String::from);
    let flash = if new_token.is_some() {
        None
    } else if status == 409 {
        Some("Limite de tokens atingido (50). Revogue um antes de criar outro.".into())
    } else {
        Some(format!("Falha ao criar token (HTTP {status})."))
    };
    let rows = fetch_api_tokens(&st, &headers).await?;
    Ok(askama_axum::IntoResponse::into_response(
        SettingsTokensTpl {
            me,
            rows,
            new_token,
            flash,
        },
    ))
}

/// POST /settings/tokens/:id/revoke — revoke one of the caller's tokens.
async fn settings_token_revoke(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(_me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = delete_at(
        &st,
        &st.backends.auth,
        &format!("/auth/tokens/{enc}"),
        &headers,
        None,
    )
    .await?;
    Ok(Redirect::to("/settings/tokens?flash=Token+revogado").into_response())
}

// ─── impersonation (superadmin) ──────────────────────────────────────────────

/// Session cookie set by expresso-auth (the auth-client extractor reads it).
const ACCESS_COOKIE: &str = "expresso_at";
/// Backup of the operator's own token while impersonating (HttpOnly).
const ACCESS_BACKUP_COOKIE: &str = "expresso_at_orig";
/// JS-readable marker so the appnav can show the impersonation banner.
const IMPERSONATING_COOKIE: &str = "expresso_impersonating";

/// Extract one cookie's value from the request Cookie header(s).
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for raw in headers.get_all(header::COOKIE) {
        let Ok(s) = raw.to_str() else { continue };
        for pair in s.split(';') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// Append a session-cookie Set-Cookie header (HttpOnly unless `js_readable`).
fn append_session_cookie(resp: &mut Response, name: &str, value: &str, js_readable: bool) {
    let http_only = if js_readable { "" } else { " HttpOnly;" };
    if let Ok(hv) =
        header::HeaderValue::from_str(&format!("{name}={value}; Path=/;{http_only} SameSite=Lax"))
    {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

/// Append a Set-Cookie that expires (deletes) `name`.
fn append_expired_cookie(resp: &mut Response, name: &str) {
    if let Ok(hv) =
        header::HeaderValue::from_str(&format!("{name}=; Path=/; Max-Age=0; SameSite=Lax"))
    {
        resp.headers_mut().append(header::SET_COOKIE, hv);
    }
}

/// POST /admin/users/:id/impersonate — assume the target user's session
/// (superadmin only). On token-exchange success the BFF swaps the session
/// cookie to the target's token, backing up the operator's own token so
/// /impersonate/end can restore it. Without an exchange client the backend
/// only audits and returns the Keycloak admin-console URL — surfaced as flash.
async fn admin_user_impersonate(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let (status, resp) = crate::upstream::post_json_body(
        &st,
        &st.backends.auth,
        &format!("/auth/impersonate/{enc}"),
        &headers,
        None,
        &(),
    )
    .await?;
    let token = resp
        .as_ref()
        .and_then(|v| v.get("tokens"))
        .and_then(|t| t.get("access_token"))
        .and_then(|t| t.as_str())
        .map(String::from);
    let Some(token) = token else {
        // No exchange client — backend audited and returned instructions.
        let flash = if (200..300).contains(&status) {
            "Token-exchange+indispon%C3%ADvel+%E2%80%94+use+o+console+Keycloak+(ver+auditoria)"
        } else {
            "Falha+ao+personificar"
        };
        return Ok(Redirect::to(&format!("/admin/users/{enc}?flash={flash}")).into_response());
    };
    let current = cookie_value(&headers, ACCESS_COOKIE).unwrap_or_default();
    let mut out = Redirect::to("/").into_response();
    append_session_cookie(&mut out, ACCESS_BACKUP_COOKIE, &current, false);
    append_session_cookie(&mut out, ACCESS_COOKIE, &token, false);
    append_session_cookie(&mut out, IMPERSONATING_COOKIE, "1", true);
    Ok(out)
}

/// POST /impersonate/end — restore the operator's own session from the
/// backup cookie and notify the auth service (audit trail).
async fn impersonation_end(State(st): State<AppState>, headers: HeaderMap) -> WebResult<Response> {
    // Audit first (the current cookie still identifies the impersonated
    // session); restoring the cookie happens in this response.
    let _ = crate::upstream::post_json_body(
        &st,
        &st.backends.auth,
        "/auth/impersonate/end",
        &headers,
        None,
        &(),
    )
    .await;
    let mut out = Redirect::to("/").into_response();
    if let Some(orig) = cookie_value(&headers, ACCESS_BACKUP_COOKIE) {
        append_session_cookie(&mut out, ACCESS_COOKIE, &orig, false);
    }
    append_expired_cookie(&mut out, ACCESS_BACKUP_COOKIE);
    append_expired_cookie(&mut out, IMPERSONATING_COOKIE);
    Ok(out)
}

// ─── webpush (service worker + subscription) ─────────────────────────────────

/// GET /sw.js — the service worker, served from the root path so its scope
/// covers the whole app (ServeDir under /static would restrict it).
async fn service_worker_js() -> Response {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../static/sw.js"),
    )
        .into_response()
}

/// GET /notifications/vapid-key — the VAPID public key for
/// `pushManager.subscribe`. `{key:null}` when webpush isn't configured
/// (the backend answers 404 then).
async fn notifications_vapid_key(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let v = match get_json::<serde_json::Value>(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/push/vapid-public-key",
        &headers,
        Some((&t, &u)),
    )
    .await
    {
        Ok(Some(v)) => v,
        _ => serde_json::json!({ "key": null }),
    };
    Ok(json_response(&v))
}

#[derive(Deserialize)]
struct PushSubscribeForm {
    endpoint: String,
    p256dh: String,
    auth: String,
}

/// POST /notifications/push/subscribe — register this browser's subscription.
async fn notifications_push_subscribe(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    axum::Json(f): axum::Json<PushSubscribeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/push",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "endpoint": f.endpoint, "p256dh": f.p256dh, "auth": f.auth }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

#[derive(Deserialize)]
struct PushUnsubscribeForm {
    endpoint: String,
}

/// POST /notifications/push/unsubscribe — drop this browser's subscription
/// (proxies DELETE-with-body).
async fn notifications_push_unsubscribe(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    axum::Json(f): axum::Json<PushUnsubscribeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = delete_json(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/push",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "endpoint": f.endpoint }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

// ─── notification bell tray (server-backed) ──────────────────────────────────

/// GET /notifications/list — the caller's unread, non-snoozed notifications
/// as JSON for the bell tray ({notifications:[…]} passthrough).
async fn notifications_list_api(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications?limit=20",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_else(|| serde_json::json!({ "notifications": [] }));
    Ok(json_response(&v))
}

/// POST /notifications/:id/read — mark one notification read (proxies PATCH).
async fn notification_read_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = patch_json(
        &st,
        &st.backends.notifications,
        &format!("/api/v1/notifications/{enc}/read"),
        &headers,
        Some((&t, &u)),
        &(),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

/// POST /notifications/read-all — mark every notification read.
async fn notifications_read_all_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    let (t, u) = ctx_of(&me);
    let status = patch_json(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/read-all",
        &headers,
        Some((&t, &u)),
        &(),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
}

#[derive(Deserialize)]
struct NotifSnoozeForm {
    /// Snooze window in hours (backend validates 1..=720).
    hours: i64,
}

/// POST /notifications/:id/snooze — hide a notification for N hours.
async fn notification_snooze_action(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<NotifSnoozeForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !(1..=720).contains(&f.hours) {
        return Ok((StatusCode::BAD_REQUEST, "hours must be 1..=720").into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = patch_json(
        &st,
        &st.backends.notifications,
        &format!("/api/v1/notifications/{enc}/snooze"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "hours": f.hours }),
    )
    .await?;
    Ok(StatusCode::from_u16(status)
        .unwrap_or(StatusCode::BAD_GATEWAY)
        .into_response())
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
    fn mfa_factor_rows_maps_kc_credentials() {
        let v = serde_json::json!({"user_id": "u", "factors": [
            {"id": "c1", "type": "otp", "user_label": "Meu celular", "created_date": 1765295400000i64},
            {"id": "c2", "type": "webauthn"}
        ]});
        let rows = mfa_factor_rows(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, "otp");
        assert_eq!(rows[0].label, "Meu celular");
        assert_eq!(rows[0].created, "2025-12-09 15:50");
        assert_eq!(rows[1].kind, "webauthn");
        assert!(rows[1].label.is_empty());
        assert!(rows[1].created.is_empty());
        assert!(mfa_factor_rows(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn cookie_value_parses_multi_pair_headers() {
        let mut h = HeaderMap::new();
        h.append(
            header::COOKIE,
            "theme=dark; expresso_at = abc.def.ghi ;broken"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            cookie_value(&h, "expresso_at").as_deref(),
            Some("abc.def.ghi")
        );
        assert_eq!(cookie_value(&h, "theme").as_deref(), Some("dark"));
        assert_eq!(cookie_value(&h, "missing"), None);
    }

    #[test]
    fn api_token_row_maps_backend_shape() {
        let v = serde_json::json!({
            "id": "11111111-1111-1111-1111-111111111111",
            "name": "backup script",
            "created_at": "2026-06-09T10:30:00Z",
            "last_used_at": null,
            "expires_at": "2026-09-07T10:30:00Z",
            "revoked_at": null
        });
        let r = api_token_row(&v);
        assert_eq!(r.name, "backup script");
        assert_eq!(r.created, "2026-06-09 10:30");
        assert!(r.last_used.is_empty());
        assert_eq!(r.expires, "2026-09-07 10:30");
        assert!(r.active);

        let revoked = serde_json::json!({
            "id": "x", "name": "old", "created_at": "2026-01-01T00:00:00Z",
            "revoked_at": "2026-02-01T00:00:00Z"
        });
        assert!(!api_token_row(&revoked).active);
    }

    #[test]
    fn archive_tag_search_path_picks_endpoint_by_mode() {
        assert_eq!(archive_tag_search_path("", "all", ""), None);
        assert_eq!(archive_tag_search_path(" , ", "all", "x"), None);
        assert_eq!(
            archive_tag_search_path("a, b", "all", "").as_deref(),
            Some("/api/v1/compliance/archive/tags/intersect?tags=a,b")
        );
        assert_eq!(
            archive_tag_search_path("a", "any", "ignored").as_deref(),
            Some("/api/v1/compliance/archive/tags/union?tags=a")
        );
        assert_eq!(
            archive_tag_search_path("a", "all", "x, y").as_deref(),
            Some("/api/v1/compliance/archive/tags/intersect-exclude?tags=a&exclude=x,y")
        );
        // Tags are percent-encoded individually; the CSV commas survive.
        assert_eq!(
            archive_tag_search_path("é tag", "all", "").as_deref(),
            Some("/api/v1/compliance/archive/tags/intersect?tags=%C3%A9%20tag")
        );
    }

    #[test]
    fn archive_tag_hist_rows_maps_rename_and_merge_shapes() {
        let rename = serde_json::json!({"entries": [{
            "id": "11111111-1111-1111-1111-111111111111",
            "old_tag": "hold-old", "new_tag": "hold-new",
            "renamed_count": 7, "renamed_at": "2026-06-09T10:30:00Z"
        }]});
        let rows =
            archive_tag_hist_rows(&rename, "old_tag", "new_tag", "renamed_count", "renamed_at");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].from_tag, "hold-old");
        assert_eq!(rows[0].to_tag, "hold-new");
        assert_eq!(rows[0].count, 7);
        assert_eq!(rows[0].when, "2026-06-09 10:30");

        let merge = serde_json::json!({"entries": [{
            "id": "x", "src_tag": "a", "dst_tag": "b",
            "merged_count": 2, "merged_at": "2026-01-02T03:04:05Z"
        }]});
        let rows = archive_tag_hist_rows(&merge, "src_tag", "dst_tag", "merged_count", "merged_at");
        assert_eq!(rows[0].from_tag, "a");
        assert_eq!(rows[0].to_tag, "b");
        assert_eq!(rows[0].when, "2026-01-02 03:04");

        // Missing/empty payloads degrade to no rows.
        assert!(archive_tag_hist_rows(&serde_json::Value::Null, "a", "b", "c", "d").is_empty());
    }

    #[test]
    fn task_row_status_and_helpers() {
        let mk = |status: &str, prio: i16, due: Option<&str>| crate::templates::TaskRow {
            id: "1".into(),
            summary: "x".into(),
            status: status.into(),
            priority: prio,
            due: due.map(String::from),
            rrule: None,
        };
        let recurring = |r: &str| crate::templates::TaskRow {
            rrule: Some(r.into()),
            ..mk("", 0, None)
        };
        assert_eq!(recurring("FREQ=DAILY").repeat_label(), "diária");
        assert_eq!(
            recurring("FREQ=WEEKLY;INTERVAL=2").repeat_label(),
            "semanal"
        );
        assert_eq!(recurring("FREQ=MONTHLY").repeat_label(), "mensal");
        assert_eq!(recurring("FREQ=YEARLY").repeat_label(), "recorrente");
        assert_eq!(mk("", 0, None).repeat_label(), "");
        // repeat_value round-trips the inline-edit select options.
        assert_eq!(recurring("FREQ=DAILY").repeat_value(), "daily");
        assert_eq!(recurring("FREQ=WEEKLY;INTERVAL=2").repeat_value(), "weekly");
        assert_eq!(recurring("FREQ=MONTHLY").repeat_value(), "monthly");
        assert_eq!(recurring("FREQ=YEARLY").repeat_value(), "");
        assert_eq!(mk("", 0, None).repeat_value(), "");
        assert!(mk("COMPLETED", 0, None).is_done());
        assert!(mk("CANCELLED", 0, None).is_done());
        assert!(!mk("NEEDS-ACTION", 0, None).is_done());
        assert_eq!(mk("", 1, None).priority_label(), "Alta");
        assert_eq!(mk("", 5, None).priority_label(), "Média");
        assert_eq!(mk("", 9, None).priority_label(), "Baixa");
        assert_eq!(mk("", 0, None).priority_label(), "");
        assert_eq!(
            mk("", 0, Some("2026-06-10T09:00:00Z")).due_date(),
            "2026-06-10"
        );
        assert_eq!(mk("", 0, None).due_date(), "");
    }

    #[test]
    fn drive_content_hits_parses_and_skips_idless() {
        let body = serde_json::json!({
            "hits": [
                { "file_id": "f1", "name": "Report.pdf", "snippet": "…revenue…" },
                { "name": "no-id", "snippet": "x" },
                { "file_id": "f2", "name": "Notes.txt", "snippet": "" }
            ]
        });
        let hits = drive_content_hits(&body);
        assert_eq!(hits.len(), 2); // the id-less hit is dropped
        assert_eq!(hits[0].file_id, "f1");
        assert_eq!(hits[0].name, "Report.pdf");
        assert_eq!(hits[1].file_id, "f2");
        assert!(hits[1].snippet.is_empty());
    }

    #[test]
    fn drive_content_hits_empty_when_no_hits() {
        assert!(drive_content_hits(&serde_json::json!({})).is_empty());
        assert!(drive_content_hits(&serde_json::json!({ "hits": [] })).is_empty());
    }

    #[test]
    fn reminder_minutes_sorts_dedups_and_skips_junk() {
        assert_eq!(reminder_minutes("60, 15, 15, x, -5"), vec![15, 60]);
        assert_eq!(reminder_minutes(""), Vec::<u32>::new());
        assert_eq!(reminder_minutes("  ,  "), Vec::<u32>::new());
        // caps at 10 entries
        assert_eq!(reminder_minutes("1,2,3,4,5,6,7,8,9,10,11,12").len(), 10);
    }

    #[test]
    fn require_superadmin_rejects_plain_admin() {
        let mut me = mk_me("t", "u");
        me.roles = vec!["admin".into()];
        assert!(!require_superadmin(&me));
        me.roles = vec!["superadmin".into()];
        assert!(require_superadmin(&me));
        me.roles = vec!["super_admin".into()];
        assert!(require_superadmin(&me));
    }

    #[test]
    fn payload_preview_truncates_long_json() {
        let long = serde_json::json!({ "x": "y".repeat(300) });
        let p = payload_preview(&long);
        assert!(p.chars().count() <= 161);
        assert!(p.ends_with('…'));
        // short payloads pass through verbatim
        let short = serde_json::json!({ "k": 1 });
        assert_eq!(payload_preview(&short), r#"{"k":1}"#);
    }

    #[test]
    fn dlq_entry_from_json_maps_fields_and_defaults() {
        let v = serde_json::json!({
            "id": "abc", "kind": "new_mail", "attempts": 5,
            "last_error": "timeout", "failed_at": "2026-06-03T10:00:00Z",
            "payload": { "n": 1 }
        });
        let e = dlq_entry_from_json(&v);
        assert_eq!(e.id, "abc");
        assert_eq!(e.kind, "new_mail");
        assert_eq!(e.attempts, 5);
        assert_eq!(e.last_error, "timeout");
        assert_eq!(e.payload_preview, r#"{"n":1}"#);
        // absent string fields default to empty (e.g. tenant_id null)
        assert_eq!(e.tenant_id, "");
    }

    #[test]
    fn dlq_redirect_flash_reflects_status() {
        let ok = dlq_redirect(200, "feito");
        assert_eq!(ok.status(), StatusCode::SEE_OTHER);
        let loc = ok.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.starts_with("/admin/dlq?flash="));
        assert!(loc.contains("feito"));
        let bad = dlq_redirect(503, "feito");
        let loc = bad.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.contains("503"));
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
    fn summarize_conditions_one_and_many() {
        let one = serde_json::json!([{"field":"from","op":"contains","value":"x@y.com"}]);
        assert_eq!(
            summarize_conditions(&one, "and"),
            "from contains \"x@y.com\""
        );
        let many = serde_json::json!([
            {"field":"from","op":"contains","value":"a"},
            {"field":"subject","op":"equals","value":"b"}
        ]);
        assert_eq!(
            summarize_conditions(&many, "or"),
            "from contains \"a\" ou +1"
        );
        assert_eq!(
            summarize_conditions(&serde_json::json!([]), "and"),
            "qualquer mensagem"
        );
    }

    #[test]
    fn summarize_actions_known_types() {
        let mv = serde_json::json!([{"type":"move_to_folder","params":{"folder":"Fin"}}]);
        assert_eq!(summarize_actions(&mv), "mover para \"Fin\"");
        let fl = serde_json::json!([{"type":"add_flag","params":{"flag":"\\Flagged"}}]);
        assert_eq!(summarize_actions(&fl), "marcar \"\\Flagged\"");
        assert_eq!(summarize_actions(&serde_json::json!([])), "nenhuma ação");
    }

    #[test]
    fn hhmm_of_rfc3339_extracts_time() {
        assert_eq!(hhmm_of_rfc3339("2026-06-10T14:30:00Z"), "14:30");
        assert_eq!(hhmm_of_rfc3339("2026-06-10T09:05:00-03:00"), "09:05");
        assert_eq!(hhmm_of_rfc3339("short"), "short");
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

    #[test]
    fn breakout_assign_form_deserializes_email() {
        let f: BreakoutAssignForm =
            serde_json::from_value(serde_json::json!({ "email": "a@b.com" })).expect("parse");
        assert_eq!(f.email, "a@b.com");
    }

    #[test]
    fn breakout_remove_form_deserializes_user_id() {
        let f: BreakoutRemoveForm =
            serde_json::from_value(serde_json::json!({ "user_id": "abc-123" })).expect("parse");
        assert_eq!(f.user_id, "abc-123");
    }

    #[test]
    fn drive_comment_json_parses_and_truncates_when() {
        let c: DriveCommentJson = serde_json::from_value(serde_json::json!({
            "id": "c1", "user_id": "u9", "body": "looks good",
            "created_at": "2026-06-03T15:20:30Z"
        }))
        .expect("parse");
        assert_eq!(
            (c.id.as_str(), c.user_id.as_str(), c.body.as_str()),
            ("c1", "u9", "looks good")
        );
        let when: String = c
            .created_at
            .map(|s| s.replace('T', " ").chars().take(16).collect())
            .unwrap_or_default();
        assert_eq!(when, "2026-06-03 15:20");
    }

    #[test]
    fn undo_send_payload_omits_empty_cc() {
        let p = UndoSendPayload {
            from: "me@x.com".into(),
            to: vec!["a@x.com".into()],
            cc: Vec::new(),
            subject: "Hi".into(),
            body_text: "body".into(),
            undo_seconds: 10,
        };
        let v = serde_json::to_value(&p).expect("serialize");
        assert!(v.get("cc").is_none());
        assert_eq!(v["undo_seconds"], 10);
        assert_eq!(v["to"], serde_json::json!(["a@x.com"]));
    }

    #[test]
    fn archive_stat_rows_map_label_and_count() {
        let resp = serde_json::json!({
            "senders": [{"sender": "ana@x.com", "count": 12}, {"sender": "bob@x.com", "count": 3}]
        });
        let rows: Vec<ArchiveStatRow> = resp
            .get("senders")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|it| ArchiveStatRow {
                        label: it
                            .get("sender")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(?)")
                            .to_string(),
                        count: it
                            .get("count")
                            .and_then(serde_json::Value::as_i64)
                            .unwrap_or(0),
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "ana@x.com");
        assert_eq!(rows[0].count, 12);
    }

    #[test]
    fn archive_tag_form_deserializes() {
        let f: ArchiveTagForm =
            serde_json::from_value(serde_json::json!({ "tag": "confidencial" })).expect("parse");
        assert_eq!(f.tag, "confidencial");
    }

    #[test]
    fn hold_form_splits_ids_and_defaults_tag() {
        let f: HoldForm =
            serde_json::from_value(serde_json::json!({ "ids": " a , b ,, c " })).expect("parse");
        let ids: Vec<&str> = f
            .ids
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
        let trimmed = f.hold_tag.trim();
        let tag = if trimmed.is_empty() {
            "hold-litigation"
        } else {
            trimmed
        };
        assert_eq!(tag, "hold-litigation");
    }

    #[test]
    fn archive_size_and_join_and_parse() {
        assert_eq!(archive_size(512), "512 B");
        assert_eq!(archive_size(2048), "2.0 KB");
        assert_eq!(archive_size(3 * 1024 * 1024), "3.0 MB");
        let many = serde_json::json!(["a@x.com", "b@x.com", "c@x.com", "d@x.com"]);
        assert_eq!(join_to_addrs(&many), "a@x.com, b@x.com, c@x.com, +1");
        let few = serde_json::json!(["a@x.com"]);
        assert_eq!(join_to_addrs(&few), "a@x.com");
        let a: ArchiveEntryJson = serde_json::from_value(serde_json::json!({
            "id": "e1", "subject": "Contrato", "from_addr": "ana@x.com",
            "to_addrs": ["bob@x.com"], "archived_at": "2026-05-01T10:00:00Z", "size_bytes": 2048
        }))
        .expect("parse");
        assert_eq!(a.id, "e1");
        assert_eq!(a.size_bytes, 2048);
    }

    #[test]
    fn flow_reorder_assigns_priority_by_position() {
        let f = FlowReorderForm {
            ids: " a , b ,, c ".into(),
        };
        let entries: Vec<serde_json::Value> = f
            .ids
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, id)| serde_json::json!({ "id": id, "priority": (i as i64) + 1 }))
            .collect();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], serde_json::json!({"id":"a","priority":1}));
        assert_eq!(entries[2], serde_json::json!({"id":"c","priority":3}));
    }

    #[test]
    fn bulk_summary_form_requires_summary() {
        let f: BulkSummaryForm = serde_json::from_value(serde_json::json!({
            "cal_id": "c", "summary": "Reunião semanal", "from": "2026-06-01", "to": "2026-06-30"
        }))
        .expect("parse");
        assert_eq!(f.summary, "Reunião semanal");
        assert!("  ".trim().is_empty());
    }

    #[test]
    fn bulk_location_form_defaults_empty_location() {
        let f: BulkLocationForm = serde_json::from_value(serde_json::json!({
            "cal_id": "c", "from": "2026-06-01", "to": "2026-06-30"
        }))
        .expect("parse");
        assert!(f.location.is_empty());
        let f2: BulkLocationForm = serde_json::from_value(serde_json::json!({
            "cal_id": "c", "location": "Sala 3", "from": "2026-06-01", "to": "2026-06-30"
        }))
        .expect("parse");
        assert_eq!(f2.location, "Sala 3");
    }

    #[test]
    fn bulk_status_form_parses_and_validates_whitelist() {
        let f: BulkStatusForm = serde_json::from_value(serde_json::json!({
            "cal_id": "c", "status": "CANCELLED", "from": "2026-06-01", "to": "2026-06-30"
        }))
        .expect("parse");
        assert!(matches!(
            f.status.as_str(),
            "CONFIRMED" | "TENTATIVE" | "CANCELLED"
        ));
        assert!(!matches!("BOGUS", "CONFIRMED" | "TENTATIVE" | "CANCELLED"));
    }

    #[test]
    fn bulk_move_form_deserializes_all_fields() {
        let f: CalBulkMoveForm = serde_json::from_value(serde_json::json!({
            "cal_id": "src", "dst": "dest", "from": "2026-06-01", "to": "2026-06-30"
        }))
        .expect("parse");
        assert_eq!(f.cal_id, "src");
        assert_eq!(f.dst, "dest");
        assert_ne!(f.cal_id, f.dst);
    }

    #[test]
    fn histogram_label_truncates_by_bucket() {
        assert_eq!(histogram_label("2026-06-03T00:00:00Z", "day"), "2026-06-03");
        assert_eq!(
            histogram_label("2026-06-03T00:00:00Z", "week"),
            "2026-06-03"
        );
        assert_eq!(histogram_label("2026-06-03T00:00:00Z", "month"), "2026-06");
        let p: HistogramPoint =
            serde_json::from_value(serde_json::json!({ "ts": "2026-06-03T00:00:00Z", "count": 4 }))
                .expect("parse");
        assert_eq!(p.count, 4);
        assert_eq!(p.ts.as_deref(), Some("2026-06-03T00:00:00Z"));
    }

    #[test]
    fn bulk_delete_event_json_parse_and_recurring() {
        let e: BulkDeleteEventJson = serde_json::from_value(serde_json::json!({
            "summary": "Daily", "dtstart": "2026-06-05T08:00:00Z", "rrule": "FREQ=DAILY"
        }))
        .expect("parse");
        assert_eq!(e.summary.as_deref(), Some("Daily"));
        assert!(e.rrule.is_some());
        let when: String = e
            .dtstart
            .map(|s| s.replace('T', " ").chars().take(16).collect())
            .unwrap_or_default();
        assert_eq!(when, "2026-06-05 08:00");
        let plain: BulkDeleteEventJson =
            serde_json::from_value(serde_json::json!({ "summary": "One-off" })).expect("parse");
        assert!(plain.rrule.is_none());
    }

    #[test]
    fn conflict_span_label_and_parse() {
        let p: ConflictPairJson = serde_json::from_value(serde_json::json!({
            "a_summary": "Standup", "a_dtstart": "2026-06-04T09:00:00Z", "a_dtend": "2026-06-04T09:30:00Z",
            "b_summary": "1:1", "b_dtstart": "2026-06-04T09:15:00Z", "b_dtend": "2026-06-04T09:45:00Z"
        }))
        .expect("parse");
        assert_eq!(p.a_summary.as_deref(), Some("Standup"));
        assert_eq!(span_label(&p.a_dtstart, &p.a_dtend), "09:00–09:30");
        assert_eq!(span_label(&p.b_dtstart, &p.b_dtend), "09:15–09:45");
    }

    #[test]
    fn counter_proposal_parses_and_fmt_dt_truncates() {
        let p: CounterProposal = serde_json::from_value(serde_json::json!({
            "id": "c1", "event_id": "e1", "attendee_email": "a@b.com",
            "proposed_dtstart": "2026-06-10T14:30:00Z"
        }))
        .expect("parse");
        assert_eq!(p.attendee_email, "a@b.com");
        assert_eq!(fmt_dt(p.proposed_dtstart), "2026-06-10 14:30");
        assert_eq!(fmt_dt(p.proposed_dtend), "");
    }

    #[test]
    fn note_tags_form_splits_csv_trimming_blanks() {
        let f: NoteTagsForm =
            serde_json::from_value(serde_json::json!({ "tags": " a, b ,,\nc " })).expect("parse");
        let tags: Vec<String> = f
            .tags
            .split([',', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        assert_eq!(tags, vec!["a", "b", "c"]);
    }

    #[test]
    fn tag_rename_and_merge_forms_deserialize() {
        let r: TagRenameForm =
            serde_json::from_value(serde_json::json!({ "tag": "wip", "new": "in-progress" }))
                .expect("parse");
        assert_eq!((r.tag.as_str(), r.new.as_str()), ("wip", "in-progress"));
        let m: TagMergeForm =
            serde_json::from_value(serde_json::json!({ "tag": "todo", "into": "tasks" }))
                .expect("parse");
        assert_eq!((m.tag.as_str(), m.into.as_str()), ("todo", "tasks"));
    }

    #[test]
    fn snooze_record_deserializes_id_and_until() {
        let r: SnoozeRecord = serde_json::from_value(serde_json::json!({
            "message_id": "m-1", "snooze_until": "2026-06-04T09:30:00Z"
        }))
        .expect("parse");
        assert_eq!(r.message_id, "m-1");
        let wake: String = r.snooze_until.replace('T', " ").chars().take(16).collect();
        assert_eq!(wake, "2026-06-04 09:30");
    }

    #[test]
    fn thread_state_defaults_false_when_absent() {
        let s: ThreadState = serde_json::from_value(serde_json::json!({})).expect("parse");
        assert!(!s.muted && !s.pinned);
        let s2: ThreadState =
            serde_json::from_value(serde_json::json!({ "muted": true, "pinned": false }))
                .expect("parse");
        assert!(s2.muted && !s2.pinned);
    }

    #[test]
    fn thread_flag_form_defaults_off() {
        let f: ThreadFlagForm = serde_json::from_value(serde_json::json!({})).expect("parse");
        assert!(!f.on);
        let f2: ThreadFlagForm =
            serde_json::from_value(serde_json::json!({ "on": true })).expect("parse");
        assert!(f2.on);
    }

    #[test]
    fn transcript_search_query_defaults_empty() {
        let q: TranscriptSearchQuery =
            serde_json::from_value(serde_json::json!({})).expect("parse");
        assert!(q.q.is_empty());
        let q2: TranscriptSearchQuery =
            serde_json::from_value(serde_json::json!({ "q": "hello" })).expect("parse");
        assert_eq!(q2.q, "hello");
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
    // MFA factors — superadmin only; best-effort (auth answers 503 when no
    // Keycloak admin client is configured → section shows as unavailable).
    let mfa = if require_superadmin(&me) {
        let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
        match get_json::<serde_json::Value>(
            &st,
            &st.backends.auth,
            &format!("/auth/admin/users/{enc}/mfa"),
            &headers,
            None,
        )
        .await
        {
            Ok(Some(v)) => Some(mfa_factor_rows(&v)),
            _ => None,
        }
    } else {
        None
    };
    let flash = extract_flash(&uri);
    Ok(askama_axum::IntoResponse::into_response(
        AdminUserDetailTpl {
            me,
            user,
            logins,
            mfa,
            flash,
        },
    ))
}

/// Map the auth MFA list ({factors:[{id,type,user_label?,created_date?}]},
/// created_date in epoch millis) into table rows.
fn mfa_factor_rows(v: &serde_json::Value) -> Vec<MfaFactorRow> {
    v.get("factors")
        .and_then(|f| f.as_array())
        .map(|arr| {
            arr.iter()
                .map(|c| MfaFactorRow {
                    id: c
                        .get("id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    kind: c
                        .get("type")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    label: c
                        .get("user_label")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    created: c
                        .get("created_date")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|ms| time::OffsetDateTime::from_unix_timestamp(ms / 1000).ok())
                        .map(|dt| {
                            format!(
                                "{:04}-{:02}-{:02} {:02}:{:02}",
                                dt.year(),
                                dt.month() as u8,
                                dt.day(),
                                dt.hour(),
                                dt.minute()
                            )
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Deserialize)]
struct MfaRequireForm {
    /// "totp" or "webauthn".
    factor: String,
}

/// POST /admin/users/:id/mfa/require — email the user a required-action to
/// enroll an MFA factor on next login (superadmin only).
async fn admin_user_mfa_require(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<MfaRequireForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let factor = match f.factor.as_str() {
        "webauthn" => "webauthn",
        _ => "totp",
    };
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let (status, _) = crate::upstream::post_json_body(
        &st,
        &st.backends.auth,
        &format!("/auth/admin/users/{enc}/mfa/require"),
        &headers,
        None,
        &serde_json::json!({ "factor": factor }),
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Inscri%C3%A7%C3%A3o+MFA+solicitada+por+e-mail"
    } else {
        "Falha+ao+exigir+MFA+(Keycloak+admin+indispon%C3%ADvel%3F)"
    };
    Ok(Redirect::to(&format!("/admin/users/{enc}?flash={flash}")).into_response())
}

/// POST /admin/users/:id/mfa/:cred/delete — remove (reset) one MFA factor.
async fn admin_user_mfa_delete(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path((id, cred)): Path<(String, String)>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let cenc = utf8_percent_encode(&cred, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.auth,
        &format!("/auth/admin/users/{enc}/mfa/{cenc}"),
        &headers,
        None,
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Fator+MFA+removido"
    } else {
        "Falha+ao+remover+fator+MFA"
    };
    Ok(Redirect::to(&format!("/admin/users/{enc}?flash={flash}")).into_response())
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

// ─── /admin/dlq (notification dead-letter queue) ───────────────────────────────

/// The notifications DLQ ops require a super-admin (not a plain tenant `admin`),
/// matching the backend `require_dlq_admin` gate. A page shown to a non-super
/// admin would only 403 at the backend, so gate the UI the same way.
fn require_superadmin(me: &Me) -> bool {
    me.roles
        .iter()
        .any(|r| r == "superadmin" || r == "super_admin")
}

#[derive(Deserialize)]
struct DlqPageQuery {
    kind: Option<String>,
    flash: Option<String>,
}

/// One-line preview of a DLQ payload for the table cell (compact JSON, capped).
fn payload_preview(payload: &serde_json::Value) -> String {
    let s = payload.to_string();
    if s.chars().count() > 160 {
        let mut out: String = s.chars().take(160).collect();
        out.push('…');
        out
    } else {
        s
    }
}

fn dlq_entry_from_json(v: &serde_json::Value) -> DlqEntry {
    let str_of = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or_default()
            .to_string()
    };
    DlqEntry {
        id: str_of("id"),
        tenant_id: str_of("tenant_id"),
        user_id: str_of("user_id"),
        kind: str_of("kind"),
        attempts: v
            .get("attempts")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        last_error: str_of("last_error"),
        failed_at: str_of("failed_at"),
        payload_preview: v.get("payload").map(payload_preview).unwrap_or_default(),
    }
}

async fn admin_dlq_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Query(q): Query<DlqPageQuery>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let filter_kind = q.kind.unwrap_or_default();
    let list_path = if filter_kind.is_empty() {
        "/api/v1/notifications/dlq?limit=200".to_string()
    } else {
        let enc = utf8_percent_encode(&filter_kind, NON_ALPHANUMERIC);
        format!("/api/v1/notifications/dlq?limit=200&kind={enc}")
    };
    let list = get_json::<serde_json::Value>(
        &st,
        &st.backends.notifications,
        &list_path,
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let entries: Vec<DlqEntry> = list
        .get("items")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(dlq_entry_from_json).collect())
        .unwrap_or_default();

    let stats = get_json::<serde_json::Value>(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/dlq/stats",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let total = stats
        .get("total")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let by_kind: Vec<DlqKindCount> = stats
        .get("by_kind")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|r| DlqKindCount {
                    kind: r
                        .get("kind")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    count: r
                        .get("count")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(askama_axum::IntoResponse::into_response(AdminDlqTpl {
        me,
        total,
        entries,
        by_kind,
        filter_kind,
        flash: q.flash,
    }))
}

/// Resolve a flash message for the DLQ redirect from an upstream status code.
fn dlq_redirect(status: u16, ok_msg: &str) -> Response {
    let flash = if (200..300).contains(&status) {
        ok_msg.to_string()
    } else {
        format!("Falha (HTTP {status})")
    };
    let enc = utf8_percent_encode(&flash, NON_ALPHANUMERIC);
    Redirect::to(&format!("/admin/dlq?flash={enc}")).into_response()
}

async fn admin_dlq_retry(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = post_empty(
        &st,
        &st.backends.notifications,
        &format!("/api/v1/notifications/dlq/{enc}/retry"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(dlq_redirect(status, "Notificação reenviada"))
}

async fn admin_dlq_delete(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let status = delete_at(
        &st,
        &st.backends.notifications,
        &format!("/api/v1/notifications/dlq/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(dlq_redirect(status, "Entrada apagada"))
}

async fn admin_dlq_purge(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let status = delete_at(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/dlq",
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(dlq_redirect(status, "Fila esvaziada"))
}

async fn admin_dlq_retry_all(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_superadmin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let status = post_empty(
        &st,
        &st.backends.notifications,
        "/api/v1/notifications/dlq/retry-all",
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(dlq_redirect(status, "Reenvio de todas disparado"))
}

// ─── /admin/resources (bookable calendar resources) ────────────────────────────

/// GET /admin/resources — list the tenant's bookable resources (rooms/equipment).
/// Viewing is any-user on the backend, but registry edits are admin-gated, so the
/// management page itself is admin-only.
async fn admin_resources_page(
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
    let body = get_json::<serde_json::Value>(
        &st,
        &st.backends.calendar,
        "/api/v1/resources",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let resources: Vec<Resource> = body
        .get("resources")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok(askama_axum::IntoResponse::into_response(
        AdminResourcesTpl {
            me,
            resources,
            flash: extract_flash(&uri),
        },
    ))
}

#[derive(Deserialize)]
struct ResourceForm {
    email: String,
    name: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    capacity: String,
}

/// POST /admin/resources — register a bookable resource (admin only).
async fn admin_resource_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<ResourceForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (email, name) = (f.email.trim(), f.name.trim());
    if email.is_empty() || name.is_empty() {
        return Ok(
            Redirect::to("/admin/resources?flash=E-mail+e+nome+obrigat%C3%B3rios").into_response(),
        );
    }
    let mut payload = serde_json::json!({ "email": email, "name": name });
    if !f.kind.trim().is_empty() {
        payload["kind"] = serde_json::json!(f.kind.trim());
    }
    if let Ok(cap) = f.capacity.trim().parse::<i32>() {
        if cap > 0 {
            payload["capacity"] = serde_json::json!(cap);
        }
    }
    let (t, u) = ctx_of(&me);
    let status = post_json(
        &st,
        &st.backends.calendar,
        "/api/v1/resources",
        &headers,
        Some((&t, &u)),
        &payload,
    )
    .await?;
    let flash = if (200..300).contains(&status) {
        "Recurso registrado"
    } else {
        "Falha ao registrar (e-mail já existe?)"
    };
    let enc = utf8_percent_encode(flash, NON_ALPHANUMERIC);
    Ok(Redirect::to(&format!("/admin/resources?flash={enc}")).into_response())
}

/// POST /admin/resources/:id/delete — unregister a resource (admin only).
async fn admin_resource_delete(
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
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = delete_at(
        &st,
        &st.backends.calendar,
        &format!("/api/v1/resources/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/admin/resources?flash=Recurso+removido").into_response())
}

// ─── /admin/tenants/:id/usage ────────────────────────────────────────────────

/// GET /admin/tenants/:id/usage — per-tenant usage report (users, mail,
/// drive, calendar, contacts) from the expresso-admin service. The backend
/// scopes access: tenant-admins see their own tenant, superadmins any.
async fn admin_tenant_usage_page(
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
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let v = get_json::<serde_json::Value>(
        &st,
        &st.backends.admin,
        &format!("/api/v1/admin/tenants/{enc}/usage"),
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default();
    let count = |key: &str| -> String {
        v.get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .to_string()
    };
    let size = |key: &str| -> String {
        archive_size(v.get(key).and_then(serde_json::Value::as_i64).unwrap_or(0))
    };
    let rows = vec![
        TenantUsageRow {
            label: "👥 Usuários".into(),
            value: count("user_count"),
        },
        TenantUsageRow {
            label: "✉ Mensagens de e-mail".into(),
            value: count("message_count"),
        },
        TenantUsageRow {
            label: "✉ Tamanho das caixas".into(),
            value: size("mailbox_size_bytes"),
        },
        TenantUsageRow {
            label: "💾 Arquivos no drive".into(),
            value: count("file_count"),
        },
        TenantUsageRow {
            label: "💾 Tamanho do drive".into(),
            value: size("file_size_bytes"),
        },
        TenantUsageRow {
            label: "📅 Eventos de agenda".into(),
            value: count("calendar_event_count"),
        },
        TenantUsageRow {
            label: "👤 Contatos".into(),
            value: count("contact_count"),
        },
    ];
    Ok(askama_axum::IntoResponse::into_response(
        AdminTenantUsageTpl {
            me,
            tenant_id: id,
            rows,
        },
    ))
}

// ─── /admin/retention (compliance archive retention) ────────────────────────

/// GET /admin/retention — tenant default retention + per-folder policies.
async fn admin_retention_page(
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
    let default_days = get_json::<serde_json::Value>(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/retention",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .and_then(|v| v.get("retain_days").and_then(serde_json::Value::as_i64))
    .unwrap_or(365);
    let policies = get_json::<Vec<serde_json::Value>>(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/retention-policies",
        &headers,
        Some((&t, &u)),
    )
    .await?
    .unwrap_or_default()
    .into_iter()
    .map(|p| RetentionPolicyRow {
        id: p
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        folder: p
            .get("folder_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        retain_days: p
            .get("retain_days")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
        action: p
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("delete")
            .to_string(),
        enabled: p
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
    .collect();
    Ok(askama_axum::IntoResponse::into_response(
        AdminRetentionTpl {
            me,
            default_days,
            policies,
            flash: extract_flash(&uri),
        },
    ))
}

#[derive(Deserialize)]
struct RetentionDefaultForm {
    retain_days: i64,
}

/// POST /admin/retention/default — set the tenant-wide archive retention.
async fn admin_retention_set_default(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<RetentionDefaultForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    if f.retain_days <= 0 || f.retain_days > 36500 {
        return Ok((StatusCode::BAD_REQUEST, "retain_days fora do intervalo").into_response());
    }
    let _ = put_json(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/retention",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "retain_days": f.retain_days }),
    )
    .await?;
    Ok(Redirect::to("/admin/retention?flash=Reten%C3%A7%C3%A3o+padr%C3%A3o+salva").into_response())
}

#[derive(Deserialize)]
struct RetentionPolicyForm {
    #[serde(default)]
    folder_name: String,
    retain_days: i64,
}

/// POST /admin/retention/policies — create a per-folder retention policy
/// (empty folder = all folders).
async fn admin_retention_policy_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Form(f): Form<RetentionPolicyForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    if f.retain_days <= 0 {
        return Ok((StatusCode::BAD_REQUEST, "retain_days deve ser > 0").into_response());
    }
    let folder = f.folder_name.trim();
    let folder_json = if folder.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::Value::String(folder.to_string())
    };
    let _ = post_json(
        &st,
        &st.backends.compliance,
        "/api/v1/compliance/retention-policies",
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "folder_name": folder_json, "retain_days": f.retain_days }),
    )
    .await?;
    Ok(Redirect::to("/admin/retention?flash=Pol%C3%ADtica+criada").into_response())
}

#[derive(Deserialize)]
struct RetentionToggleForm {
    /// Desired state: "true" enables, anything else disables.
    on: String,
}

/// POST /admin/retention/policies/:id/toggle — enable/disable a policy.
async fn admin_retention_policy_toggle(
    State(st): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
    Path(id): Path<String>,
    Form(f): Form<RetentionToggleForm>,
) -> WebResult<Response> {
    let Some(me) = require_me(&st, &headers).await? else {
        return Ok(login_redirect(&uri).into_response());
    };
    if !require_admin(&me) {
        return Ok((StatusCode::FORBIDDEN, "Acesso negado").into_response());
    }
    let (t, u) = ctx_of(&me);
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = patch_json(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/retention-policies/{enc}"),
        &headers,
        Some((&t, &u)),
        &serde_json::json!({ "enabled": f.on == "true" }),
    )
    .await?;
    Ok(Redirect::to("/admin/retention?flash=Pol%C3%ADtica+atualizada").into_response())
}

/// POST /admin/retention/policies/:id/delete — remove a policy.
async fn admin_retention_policy_delete(
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
    let enc = utf8_percent_encode(&id, NON_ALPHANUMERIC);
    let _ = delete_at(
        &st,
        &st.backends.compliance,
        &format!("/api/v1/compliance/retention-policies/{enc}"),
        &headers,
        Some((&t, &u)),
    )
    .await?;
    Ok(Redirect::to("/admin/retention?flash=Pol%C3%ADtica+removida").into_response())
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
