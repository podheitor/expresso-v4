//! Askama templates.
#![allow(dead_code)]

use askama::Template;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct MfaInfo {
    #[serde(default)]
    pub totp: bool,
    #[serde(default)]
    pub webauthn: bool,
    #[serde(default)]
    pub amr: Vec<String>,
    #[serde(default)]
    pub acr: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Me {
    pub user_id: String,
    pub tenant_id: String,
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub expires_at: i64,
    #[serde(default)]
    pub mfa: Option<MfaInfo>,
}

impl Me {
    /// True if the user holds an admin-tier role. Exposed as a method because
    /// askama templates can't evaluate closures (`roles.iter().any(|r| …)`).
    pub fn is_admin(&self) -> bool {
        self.roles.iter().any(|r| r == "admin" || r == "superadmin")
    }

    /// True only for super-admins (impersonation gate).
    pub fn is_superadmin(&self) -> bool {
        self.roles.iter().any(|r| r == "superadmin")
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Folder {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub special_use: Option<String>,
    #[serde(default)]
    pub message_count: i64,
    #[serde(default)]
    pub unseen_count: i64,
}

impl Folder {
    pub fn icon(&self) -> &'static str {
        match self.special_use.as_deref() {
            Some("\\Inbox") => "📥",
            Some("\\Sent") => "📤",
            Some("\\Drafts") => "📝",
            Some("\\Trash") => "🗑",
            Some("\\Junk") => "🚫",
            _ => "📁",
        }
    }
    /// A user folder (not a system/special-use mailbox) may be renamed/deleted.
    pub fn manageable(&self) -> bool {
        self.special_use.as_deref().is_none_or(str::is_empty)
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct MessageListItem {
    pub id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub from_addr: Option<String>,
    #[serde(default)]
    pub from_name: Option<String>,
    #[serde(default)]
    pub preview_text: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub has_attachments: bool,
}

impl MessageListItem {
    pub fn is_unread(&self) -> bool {
        !self.flags.iter().any(|f| f == "\\Seen")
    }
    pub fn sender_display(&self) -> &str {
        self.from_name
            .as_deref()
            .or(self.from_addr.as_deref())
            .unwrap_or("—")
    }
    pub fn subject_display(&self) -> &str {
        self.subject.as_deref().unwrap_or("(sem assunto)")
    }
    pub fn preview_display(&self) -> &str {
        self.preview_text.as_deref().unwrap_or("")
    }
    pub fn date_display(&self) -> &str {
        self.date.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct MessageDetail {
    pub id: String,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub from_addr: Option<String>,
    #[serde(default)]
    pub from_name: Option<String>,
    #[serde(default)]
    pub to_addrs: serde_json::Value,
    #[serde(default)]
    pub cc_addrs: serde_json::Value,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub body_text: Option<String>,
    #[serde(default)]
    pub body_html: Option<String>,
    #[serde(default)]
    pub has_attachments: bool,
}

// ───── Templates ─────────────────────────────────────────────────────────────

/// One hit in the unified search results, normalised across apps.
pub struct SearchHit {
    pub text: String,
    pub href: String,
}

/// A group of hits from one app (Mail, Drive, …) for the unified search page.
pub struct SearchGroup {
    pub label: String,
    pub icon: String,
    pub hits: Vec<SearchHit>,
}

/// One category facet chip (a source with at least one hit) on the unified
/// search page.
pub struct SearchFacet {
    pub label: String,
    pub icon: String,
    pub count: usize,
}

/// A note as shown in the webmail Notes screen (subset of the notes service's model).
#[derive(serde::Deserialize)]
pub struct Note {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub notebook_id: Option<String>,
}

/// A notes notebook (folder). Notes reference one via `notebook_id`.
#[derive(Debug, Clone, Deserialize)]
pub struct Notebook {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Template)]
#[template(path = "notes.html")]
pub struct NotesTpl {
    pub me: Me,
    pub notes: Vec<Note>,
    /// The note open in the editor pane (when one is selected), else None.
    pub selected: Option<Note>,
    pub notebooks: Vec<Notebook>,
    /// Active notebook filter: a notebook id, the literal "none", or empty (all).
    pub current_notebook: String,
}

#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTpl {
    pub me: Me,
    pub query: String,
    /// Percent-encoded query, for building the facet-chip hrefs safely.
    pub query_enc: String,
    pub groups: Vec<SearchGroup>,
    pub total: usize,
    /// Category chips (one per source with hits), counted over the full set.
    pub facets: Vec<SearchFacet>,
    /// Active category filter (a group label), or empty for all.
    pub active_type: String,
}

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTpl {
    pub login_url: String,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "me.html")]
pub struct MeTpl {
    pub me: Me,
    pub logout_url: String,
}

#[derive(Template)]
#[template(path = "security.html")]
pub struct SecurityTpl {
    pub me: Me,
    pub kc_account: String,
}

#[derive(Template)]
#[template(path = "mail_list.html")]
pub struct MailListTpl {
    pub me: Me,
    pub folders: Vec<Folder>,
    pub selected: String,
    pub messages: Vec<MessageListItem>,
    pub detail: Option<MessageDetail>,
    pub selected_id: Option<String>,
    pub page: u32,
    pub has_next: bool,
    /// When viewing a delegated mailbox, the owner's email (for the banner).
    pub viewing_as: Option<String>,
    /// The delegated owner's user id, for carrying `obo` on folder links.
    pub obo: Option<String>,
    /// The user's flag presets, offered as quick-apply buttons on the open
    /// message. Empty on the list-only view.
    pub flag_presets: Vec<FlagPreset>,
}

impl MailListTpl {
    /// `&obo=<id>` suffix for in-mailbox links when viewing a delegated box.
    pub fn obo_suffix(&self) -> String {
        match self.obo.as_deref() {
            Some(id) if !id.is_empty() => format!("&obo={id}"),
            _ => String::new(),
        }
    }
}

#[derive(Template)]
#[template(path = "mail_compose.html")]
pub struct MailComposeTpl {
    pub me: Me,
    pub error: Option<String>,
    pub prefill_to: String,
    pub prefill_subject: String,
    pub prefill_body: String,
    /// Additional From addresses the caller may send as (owners who granted a
    /// SEND delegation). Empty for most users → the From stays a fixed field.
    pub send_as: Vec<String>,
}

#[derive(Template)]
#[template(path = "mail_thread.html")]
pub struct MailThreadTpl {
    pub me: Me,
    pub folders: Vec<Folder>,
    pub thread_id: String,
    pub messages: Vec<MessageListItem>,
    pub subject: String,
    pub muted: bool,
    pub pinned: bool,
}

/// One comment on a drive file (author email resolved for display).
pub struct DriveCommentRow {
    pub id: String,
    pub author: String,
    pub body: String,
    /// "YYYY-MM-DD HH:MM"
    pub when: String,
    /// True when the current user authored it (delete affordance).
    pub mine: bool,
}

#[derive(Template)]
#[template(path = "drive_comments.html")]
pub struct DriveCommentsTpl {
    pub me: Me,
    pub file_id: String,
    pub file_name: String,
    pub comments: Vec<DriveCommentRow>,
}

/// One snoozed message on the `/mail/snoozed` page.
pub struct SnoozedRow {
    pub message_id: String,
    /// "YYYY-MM-DD HH:MM" the message returns to the inbox.
    pub wake_at: String,
    pub subject: String,
    pub from: String,
}

#[derive(Template)]
#[template(path = "mail_snoozed.html")]
pub struct MailSnoozedTpl {
    pub me: Me,
    pub folders: Vec<Folder>,
    pub rows: Vec<SnoozedRow>,
}

/// One pending scheduled-send message on the `/mail/scheduled` page.
pub struct ScheduledRow {
    pub id: String,
    pub subject: String,
    pub to: String,
    /// "YYYY-MM-DD HH:MM" the message will be delivered.
    pub deliver_at: String,
}

#[derive(Template)]
#[template(path = "mail_scheduled.html")]
pub struct MailScheduledTpl {
    pub me: Me,
    pub folders: Vec<Folder>,
    pub rows: Vec<ScheduledRow>,
}

// ─── Drive ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub size_bytes: i64,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub starred_at: Option<String>,
    #[serde(default)]
    pub locked_by: Option<String>,
    #[serde(default)]
    pub locked_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.kind == "folder"
    }
    pub fn is_starred(&self) -> bool {
        self.starred_at.is_some()
    }
    pub fn is_locked(&self) -> bool {
        self.locked_at.is_some()
    }
    /// True when the file is locked by the given user id (the lock holder).
    pub fn locked_by_me(&self, user_id: &str) -> bool {
        self.locked_by.as_deref() == Some(user_id)
    }
    pub fn has_expiry(&self) -> bool {
        self.expires_at.is_some()
    }
    /// "YYYY-MM-DD HH:MM" of the expiry instant, or "" when none.
    pub fn expiry_human(&self) -> String {
        self.expires_at
            .as_deref()
            .map(|s| s.replace('T', " ").chars().take(16).collect())
            .unwrap_or_default()
    }
    pub fn size_human(&self) -> String {
        if self.is_folder() {
            return "—".into();
        }
        let b = self.size_bytes as f64;
        if b < 1024.0 {
            format!("{} B", self.size_bytes)
        } else if b < 1024.0 * 1024.0 {
            format!("{:.1} KB", b / 1024.0)
        } else if b < 1024.0 * 1024.0 * 1024.0 {
            format!("{:.1} MB", b / (1024.0 * 1024.0))
        } else {
            format!("{:.1} GB", b / (1024.0 * 1024.0 * 1024.0))
        }
    }
    pub fn icon(&self) -> &'static str {
        if self.is_folder() {
            return "📁";
        }
        let mime = self.mime_type.as_deref().unwrap_or("");
        let ext = self
            .name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if mime.starts_with("image/")
            || matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "ico"
            )
        {
            return "🖼";
        }
        if mime == "application/pdf" || ext == "pdf" {
            return "📕";
        }
        if mime.starts_with("video/")
            || matches!(ext.as_str(), "mp4" | "mkv" | "mov" | "avi" | "webm")
        {
            return "🎬";
        }
        if mime.starts_with("audio/")
            || matches!(ext.as_str(), "mp3" | "ogg" | "flac" | "wav" | "aac")
        {
            return "🎵";
        }
        if mime.contains("zip")
            || mime.contains("tar")
            || mime.contains("gzip")
            || matches!(ext.as_str(), "zip" | "tar" | "gz" | "bz2" | "7z" | "rar")
        {
            return "🗜";
        }
        if mime.contains("spreadsheet") || matches!(ext.as_str(), "xls" | "xlsx" | "ods" | "csv") {
            return "📊";
        }
        if mime.contains("presentation") || matches!(ext.as_str(), "ppt" | "pptx" | "odp") {
            return "📽";
        }
        if mime.contains("word")
            || mime.contains("document")
            || matches!(ext.as_str(), "doc" | "docx" | "odt" | "rtf")
        {
            return "📝";
        }
        if mime.starts_with("text/")
            || matches!(
                ext.as_str(),
                "txt"
                    | "md"
                    | "rst"
                    | "json"
                    | "yaml"
                    | "toml"
                    | "xml"
                    | "html"
                    | "css"
                    | "js"
                    | "ts"
                    | "rs"
                    | "py"
                    | "go"
                    | "sh"
            )
        {
            return "📄";
        }
        "📦"
    }
    pub fn is_editable(&self) -> bool {
        !self.is_folder() && crate::wopi::is_editable_mime(self.mime_type.as_deref())
    }
    pub fn is_previewable(&self) -> bool {
        if self.is_folder() {
            return false;
        }
        match self.mime_type.as_deref() {
            Some(m) => {
                m.starts_with("image/")
                    || m == "application/pdf"
                    || m.starts_with("text/")
                    || m == "application/json"
            }
            None => {
                let ext = self
                    .name
                    .rsplit('.')
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                matches!(
                    ext.as_str(),
                    "jpg"
                        | "jpeg"
                        | "png"
                        | "gif"
                        | "webp"
                        | "svg"
                        | "pdf"
                        | "txt"
                        | "md"
                        | "json"
                        | "csv"
                )
            }
        }
    }
    pub fn preview_kind(&self) -> &'static str {
        let m = self.mime_type.as_deref().unwrap_or("");
        if m.starts_with("image/") {
            return "image";
        }
        if m == "application/pdf" {
            return "pdf";
        }
        if m.starts_with("text/") || m == "application/json" {
            return "text";
        }
        let ext = self
            .name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" => "image",
            "pdf" => "pdf",
            _ => "text",
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct DriveQuota {
    pub max_bytes: i64,
    pub used_bytes: i64,
}

impl DriveQuota {
    pub fn percent(&self) -> i64 {
        if self.max_bytes == 0 {
            return 0;
        }
        (self.used_bytes * 100 / self.max_bytes).clamp(0, 100)
    }
    pub fn used_human(&self) -> String {
        human_size(self.used_bytes)
    }
    pub fn max_human(&self) -> String {
        human_size(self.max_bytes)
    }
}

fn human_size(n: i64) -> String {
    let b = n as f64;
    if b < 1024.0 {
        format!("{b:.0} B")
    } else if b < 1_048_576.0 {
        format!("{:.1} KB", b / 1024.0)
    } else if b < 1_073_741_824.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else {
        format!("{:.2} GB", b / 1_073_741_824.0)
    }
}

#[derive(Template)]
#[template(path = "drive.html")]
pub struct DriveTpl {
    pub me: Me,
    pub parent_id: Option<String>,
    pub files: Vec<DriveFile>,
    pub quota: Option<DriveQuota>,
    /// (id, name) pairs from root → current folder, empty when at root.
    pub folder_ancestors: Vec<(String, String)>,
}

#[derive(Template)]
#[template(path = "drive_trash.html")]
pub struct DriveTrashTpl {
    pub me: Me,
    pub files: Vec<DriveFile>,
}

#[derive(Template)]
#[template(path = "drive_starred.html")]
pub struct DriveStarredTpl {
    pub me: Me,
    pub files: Vec<DriveFile>,
}

/// One recently-modified file on the drive "Recentes" page.
pub struct DriveRecentRow {
    pub id: String,
    pub name: String,
    pub size_human: String,
    /// "YYYY-MM-DD HH:MM" last-modified.
    pub modified: String,
}

#[derive(Template)]
#[template(path = "drive_recent.html")]
pub struct DriveRecentTpl {
    pub me: Me,
    pub rows: Vec<DriveRecentRow>,
}

/// One full-text content-search hit: the matched file plus a snippet of the
/// matching passage from its extracted text.
#[derive(Debug, Clone)]
pub struct DriveContentHit {
    pub file_id: String,
    pub name: String,
    pub snippet: String,
}

#[derive(Template)]
#[template(path = "drive_content_search.html")]
pub struct DriveContentSearchTpl {
    pub me: Me,
    pub query: String,
    pub hits: Vec<DriveContentHit>,
    /// True when search isn't configured (backend 503) — show a hint instead of
    /// an empty-result message.
    pub unavailable: bool,
}

// ─── Calendar ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

impl Calendar {
    /// CSS inline color style, or empty string if no color set.
    pub fn color_style(&self) -> String {
        match &self.color {
            Some(c) if !c.is_empty() => format!("background:{};color:#fff;", c),
            _ => String::new(),
        }
    }
    pub fn dot_color(&self) -> &str {
        self.color.as_deref().unwrap_or("var(--accent)")
    }
}

#[derive(Template)]
#[template(path = "calendar.html")]
pub struct CalendarTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
}

#[derive(Template)]
#[template(path = "calendar_manage.html")]
pub struct CalendarManageTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    pub flash: Option<String>,
}

/// One event row in the agenda (list) view.
pub struct AgendaRow {
    pub id: String,
    pub calendar_id: String,
    pub summary: String,
    /// "HH:MM" start, or "dia todo".
    pub time: String,
    pub location: String,
}

/// One day section of the agenda view.
pub struct AgendaDay {
    /// "Seg, 10/06" style label.
    pub label: String,
    pub rows: Vec<AgendaRow>,
}

#[derive(Template)]
#[template(path = "calendar_agenda.html")]
pub struct CalendarAgendaTpl {
    pub me: Me,
    pub days: Vec<AgendaDay>,
}

// ─── Contacts ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct AddressBook {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Contact {
    pub id: String,
    /// Owning addressbook id — needed to build /contacts/:book/:id links from
    /// list endpoints that span books (e.g. recents).
    #[serde(default)]
    pub addressbook_id: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub given_name: Option<String>,
    #[serde(default)]
    pub family_name: Option<String>,
    #[serde(default, alias = "email_primary")]
    pub email: Option<String>,
    #[serde(default, alias = "phone_primary")]
    pub phone: Option<String>,
    #[serde(default)]
    pub organization: Option<String>,
    #[serde(default)]
    pub vcard_raw: Option<String>,
    /// vCard BDAY verbatim (may be full date, "YYYY-MM-DD", or partial
    /// "--MMDD"). Parsed into month/day for the home birthdays widget.
    #[serde(default)]
    pub birthday: Option<String>,
}

impl Contact {
    pub fn name_display(&self) -> &str {
        self.full_name.as_deref().unwrap_or("—")
    }
    pub fn email_display(&self) -> &str {
        self.email.as_deref().unwrap_or("")
    }
    pub fn phone_display(&self) -> &str {
        self.phone.as_deref().unwrap_or("")
    }
    pub fn org_display(&self) -> &str {
        self.organization.as_deref().unwrap_or("")
    }
    pub fn avatar_initial(&self) -> String {
        let name = self.full_name.as_deref().unwrap_or("");
        let ch = name.chars().next().unwrap_or('?');
        ch.to_uppercase().to_string()
    }
}

#[derive(Template)]
#[template(path = "contacts.html")]
pub struct ContactsTpl {
    pub me: Me,
    pub books: Vec<AddressBook>,
    pub selected_book: Option<String>,
    pub contacts: Vec<Contact>,
}

/// One cluster of likely-duplicate contacts (shared email or name) on the
/// duplicate-finder page.
pub struct DuplicateGroup {
    /// What they share (the normalized email or name), for the heading.
    pub key: String,
    pub contacts: Vec<Contact>,
}

#[derive(Template)]
#[template(path = "contacts_duplicates.html")]
pub struct ContactDuplicatesTpl {
    pub me: Me,
    pub book_id: String,
    pub groups: Vec<DuplicateGroup>,
}

#[derive(Template)]
#[template(path = "contacts_recents.html")]
pub struct ContactRecentsTpl {
    pub me: Me,
    /// Recently-viewed contacts, most recent first.
    pub contacts: Vec<Contact>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContactGroup {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

impl ContactGroup {
    pub fn desc_display(&self) -> &str {
        self.description.as_deref().unwrap_or("")
    }
}

#[derive(Template)]
#[template(path = "contact_groups.html")]
pub struct ContactGroupsTpl {
    pub me: Me,
    pub groups: Vec<ContactGroup>,
}

#[derive(Template)]
#[template(path = "contact_group_detail.html")]
pub struct ContactGroupDetailTpl {
    pub me: Me,
    pub group: ContactGroup,
    /// Members currently in the group.
    pub members: Vec<Contact>,
    /// Candidate contacts (from the default address book) not yet members.
    pub candidates: Vec<Contact>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ShareRow {
    pub id: String,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
}

impl ShareRow {
    pub fn is_active(&self) -> bool {
        self.revoked_at.is_none()
    }
}

#[derive(Template)]
#[template(path = "drive_share.html")]
pub struct DriveShareTpl {
    pub me: Me,
    pub file: DriveFile,
    pub shares: Vec<ShareRow>,
    pub new_url: Option<String>,
    pub new_token: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct VersionRow {
    pub id: String,
    pub version_no: i32,
    pub size_bytes: i64,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

impl VersionRow {
    pub fn size_human(&self) -> String {
        human_size(self.size_bytes)
    }
}

#[derive(Template)]
#[template(path = "drive_versions.html")]
pub struct DriveVersionsTpl {
    pub me: Me,
    pub file: DriveFile,
    pub versions: Vec<VersionRow>,
    /// Tags currently on the file.
    pub tags: Vec<String>,
}

/// A drive file tag link (the `tag` field is what we surface).
#[derive(Debug, Deserialize, Clone)]
pub struct DriveFileTag {
    pub tag: String,
}

/// A co-occurring tag pair for the notes tag-relations page.
pub struct TagPairRow {
    pub tag_a: String,
    pub tag_b: String,
    pub count: i64,
}

/// One tag with how many of the caller's notes carry it.
pub struct NoteTagStat {
    pub tag: String,
    pub count: i64,
}

#[derive(Template)]
#[template(path = "notes_tags.html")]
pub struct NotesTagsTpl {
    pub me: Me,
    pub pairs: Vec<TagPairRow>,
    /// Per-tag usage counts (most-used first), for the rename/merge panel.
    pub stats: Vec<NoteTagStat>,
}

/// One activity-log entry for an object (note/contact), pre-formatted.
pub struct ActivityRow {
    pub action: String,
    pub detail: String,
    pub when: String,
}

#[derive(Template)]
#[template(path = "notes_activity.html")]
pub struct NotesActivityTpl {
    pub me: Me,
    pub note_id: String,
    pub events: Vec<ActivityRow>,
}

#[derive(Template)]
#[template(path = "contact_activity.html")]
pub struct ContactActivityTpl {
    pub me: Me,
    pub book_id: String,
    pub contact_id: String,
    pub events: Vec<ActivityRow>,
}

/// One past vCard revision of a contact (the version-history list).
#[derive(Debug, Clone, Deserialize)]
pub struct ContactVersionRow {
    pub version_no: i32,
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub created_at: String,
}

impl ContactVersionRow {
    pub fn name(&self) -> &str {
        self.full_name.as_deref().unwrap_or("(sem nome)")
    }
}

#[derive(Template)]
#[template(path = "contact_versions.html")]
pub struct ContactVersionsTpl {
    pub me: Me,
    pub book_id: String,
    pub contact_id: String,
    pub versions: Vec<ContactVersionRow>,
    /// Highest (most recent) version number — the diff target. 0 when empty.
    pub latest: i32,
}

/// One past content revision of a note (newest first).
#[derive(Deserialize)]
pub struct NoteVersionRow {
    pub version_no: i32,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub created_at: String,
}
impl NoteVersionRow {
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            "(sem título)"
        } else {
            &self.title
        }
    }
}

#[derive(Template)]
#[template(path = "note_versions.html")]
pub struct NoteVersionsTpl {
    pub me: Me,
    pub note_id: String,
    pub versions: Vec<NoteVersionRow>,
}

/// One note shared with the caller by another user.
#[derive(Deserialize)]
pub struct SharedNoteRow {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub privilege: String,
    #[serde(default)]
    pub updated_at: String,
}
impl SharedNoteRow {
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            "(sem título)"
        } else {
            &self.title
        }
    }
    /// Human label for the grant level.
    pub fn privilege_label(&self) -> &str {
        match self.privilege.to_ascii_uppercase().as_str() {
            "ADMIN" => "Admin",
            "WRITE" => "Edição",
            _ => "Leitura",
        }
    }
    pub fn when(&self) -> String {
        self.updated_at.replace('T', " ").chars().take(16).collect()
    }
}

#[derive(Template)]
#[template(path = "notes_shared.html")]
pub struct NotesSharedTpl {
    pub me: Me,
    pub rows: Vec<SharedNoteRow>,
}

#[derive(Template)]
#[template(path = "contact_diff.html")]
pub struct ContactDiffTpl {
    pub me: Me,
    pub book_id: String,
    pub contact_id: String,
    pub from_no: i32,
    pub to_no: i32,
    /// vCard lines present in `to_no` but not `from_no`.
    pub added: Vec<String>,
    /// vCard lines present in `from_no` but not `to_no`.
    pub removed: Vec<String>,
}

#[derive(Template)]
#[template(path = "drive_activity.html")]
pub struct DriveActivityTpl {
    pub me: Me,
    pub file_id: String,
    pub file_name: String,
    pub events: Vec<ActivityRow>,
}

/// A mail flow (automation) rule summarized for display.
pub struct FlowRuleRow {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Human-readable "se <campo> <op> '<valor>'" condition summary.
    pub when: String,
    /// Human-readable action summary.
    pub then: String,
}

#[derive(Template)]
#[template(path = "flows.html")]
pub struct FlowsTpl {
    pub me: Me,
    pub rules: Vec<FlowRuleRow>,
    /// Recent webhook-action deliveries (newest first), for debugging.
    pub webhook_log: Vec<WebhookLogRow>,
}

/// One webhook delivery attempt from a flow rule's webhook action.
pub struct WebhookLogRow {
    pub url: String,
    /// HTTP status as a string, or "—".
    pub status: String,
    pub ok: bool,
    /// Error detail, or empty.
    pub error: String,
    /// "YYYY-MM-DD HH:MM"
    pub when: String,
}

/// Edit form for a single-condition, single-action flow rule (the shape the UI
/// creates). Fields are pre-selected via the `*_sel` helpers in the template.
#[derive(Template)]
#[template(path = "flow_edit.html")]
pub struct FlowEditTpl {
    pub me: Me,
    pub id: String,
    pub name: String,
    pub field: String,
    pub op: String,
    pub value: String,
    pub action: String,
    pub action_value: String,
    /// True when the rule has >1 condition or action (edit would flatten it).
    pub complex: bool,
}

/// One attendee's busy intervals (pre-formatted as HH:MM ranges) for the
/// free-busy page.
pub struct FreeBusyRow {
    pub email: String,
    /// "HH:MM–HH:MM" busy spans within the queried day.
    pub busy: Vec<String>,
}

#[derive(Template)]
#[template(path = "freebusy.html")]
pub struct FreeBusyTpl {
    pub me: Me,
    /// The queried attendees text (echoed back into the form).
    pub attendees: String,
    /// The queried date (YYYY-MM-DD), echoed back.
    pub date: String,
    /// Per-attendee busy rows; empty before the first query.
    pub rows: Vec<FreeBusyRow>,
    pub queried: bool,
}

/// One free time-slot common to every attendee, on the find-a-time page.
pub struct FreeSlotRow {
    /// "HH:MM" start, for display and the prefilled event-form dtstart.
    pub start_hhmm: String,
    /// "HH:MM" end.
    pub end_hhmm: String,
    /// "YYYY-MM-DDTHH:MM" datetime-local values to prefill the new-event form.
    pub start_local: String,
    pub end_local: String,
}

#[derive(Template)]
#[template(path = "find_time.html")]
pub struct FindTimeTpl {
    pub me: Me,
    /// Default calendar id (the link target for creating the event).
    pub cal_id: String,
    pub attendees: String,
    pub date: String,
    /// Meeting length in minutes, echoed back.
    pub duration: u32,
    /// Common free slots within working hours; empty before the first query.
    pub slots: Vec<FreeSlotRow>,
    pub queried: bool,
}

#[derive(Template)]
#[template(path = "availability.html")]
pub struct AvailabilityTpl {
    pub me: Me,
    /// The person whose availability is shown (their email).
    pub email: String,
    pub date: String,
    pub duration: u32,
    /// That person's free slots within working hours.
    pub slots: Vec<FreeSlotRow>,
    pub queried: bool,
}

/// One ranked label+count row in the e-discovery analytics report.
pub struct ArchiveStatRow {
    pub label: String,
    pub count: i64,
}

#[derive(Template)]
#[template(path = "compliance_stats.html")]
pub struct ComplianceStatsTpl {
    pub me: Me,
    pub since: String,
    pub before: String,
    pub bucket: String,
    pub senders: Vec<ArchiveStatRow>,
    pub recipients: Vec<ArchiveStatRow>,
    pub domains: Vec<ArchiveStatRow>,
    pub subjects: Vec<ArchiveStatRow>,
    /// Archiving volume over time (CSS bars), reusing HistogramBar.
    pub volume: Vec<HistogramBar>,
    /// Message-size distribution (7 fixed buckets, label = "<1KB" … ">25MB").
    pub sizes: Vec<HistogramBar>,
}

/// One past tag rename/merge on the compliance archive (the undo-history
/// lists). `from_tag`→`to_tag` covers both: old→new for renames, src→dst
/// for merges.
pub struct ArchiveTagHistRow {
    pub id: String,
    pub from_tag: String,
    pub to_tag: String,
    pub count: i64,
    /// "YYYY-MM-DD HH:MM"
    pub when: String,
}

#[derive(Template)]
#[template(path = "compliance_tags.html")]
pub struct ComplianceTagsTpl {
    pub me: Me,
    /// Per-tag usage counts (most-used first), label = tag.
    pub tags: Vec<ArchiveStatRow>,
    pub renames: Vec<ArchiveTagHistRow>,
    pub merges: Vec<ArchiveTagHistRow>,
    /// Tag pairs that co-occur on archived messages (reuses TagPairRow).
    pub pairs: Vec<TagPairRow>,
}

/// One archived message in the compliance e-discovery search results.
pub struct ArchiveRow {
    pub id: String,
    pub subject: String,
    pub from_addr: String,
    pub to_addrs: String,
    /// "YYYY-MM-DD HH:MM"
    pub archived_at: String,
    pub size_human: String,
}

#[derive(Template)]
#[template(path = "compliance_archive.html")]
pub struct ComplianceArchiveTpl {
    pub me: Me,
    /// Echoed search filters.
    pub subject: String,
    pub from_addr: String,
    pub to_addr: String,
    /// Echoed tag-search fields (CSV tags, "all"/"any", CSV exclude).
    pub tags: String,
    pub tag_mode: String,
    pub exclude: String,
    pub rows: Vec<ArchiveRow>,
    pub queried: bool,
}

/// One bucket in the event-activity histogram.
pub struct HistogramBar {
    /// Bucket label (e.g. "2026-06-03" for day, "2026-06" for month).
    pub label: String,
    pub count: i64,
    /// Bar width as a percentage of the busiest bucket (0–100).
    pub pct: u32,
}

#[derive(Template)]
#[template(path = "calendar_histogram.html")]
pub struct CalendarHistogramTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    pub cal_id: String,
    pub from: String,
    pub to: String,
    pub bucket: String,
    pub bars: Vec<HistogramBar>,
    pub total: i64,
    pub queried: bool,
}

/// One event in the bulk-delete preview list.
pub struct BulkDeleteEventRow {
    pub summary: String,
    /// "YYYY-MM-DD HH:MM"
    pub when: String,
    pub recurring: bool,
}

#[derive(Template)]
#[template(path = "calendar_bulk_delete.html")]
pub struct CalendarBulkDeleteTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    pub cal_id: String,
    pub from: String,
    pub to: String,
    pub events: Vec<BulkDeleteEventRow>,
    /// True once a preview range has been queried.
    pub previewed: bool,
    /// True when the range had more events than the preview cap.
    pub truncated: bool,
}

/// One overlapping pair of events (double-booking) within a day.
pub struct ConflictPairRow {
    pub a_summary: String,
    pub a_when: String,
    pub b_summary: String,
    pub b_when: String,
}

#[derive(Template)]
#[template(path = "calendar_conflicts.html")]
pub struct CalendarConflictsTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    /// Selected calendar id (echoed into the form), empty before first query.
    pub cal_id: String,
    /// Queried day (YYYY-MM-DD), echoed back.
    pub date: String,
    pub pairs: Vec<ConflictPairRow>,
    pub queried: bool,
}

/// One pending COUNTER proposal (attendee suggested a different time).
pub struct CounterRow {
    pub id: String,
    pub event_id: String,
    pub attendee_email: String,
    /// "YYYY-MM-DD HH:MM" proposed start, or "" when absent.
    pub proposed_start: String,
    pub proposed_end: String,
    pub comment: String,
}

#[derive(Template)]
#[template(path = "calendar_counters.html")]
pub struct CalendarCountersTpl {
    pub me: Me,
    pub rows: Vec<CounterRow>,
}

/// A tag + its file count, from the drive `/tags/stats` endpoint.
#[derive(Debug, Deserialize, Clone)]
pub struct DriveTagStat {
    pub tag: String,
    pub file_count: i64,
}

#[derive(Template)]
#[template(path = "drive_tags.html")]
pub struct DriveTagsTpl {
    pub me: Me,
    pub stats: Vec<DriveTagStat>,
}

#[derive(Template)]
#[template(path = "drive_tag_files.html")]
pub struct DriveTagFilesTpl {
    pub me: Me,
    pub tag: String,
    pub files: Vec<DriveFile>,
}

#[derive(Template)]
#[template(path = "drive_edit.html")]
pub struct DriveEditTpl {
    pub me: Me,
    pub file: DriveFile,
    pub iframe_url: String,
}

#[derive(Template)]
#[template(path = "drive_preview.html")]
pub struct DrivePreviewTpl {
    pub me: Me,
    pub file: DriveFile,
    pub download_url: String,
}

// ─── Home dashboard ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct HomeMailSummary {
    pub unread_count: i64,
    pub inbox_id: String,
}

#[derive(Debug, Clone)]
pub struct HomeEvent {
    pub id: String,
    pub calendar_id: String,
    pub summary: String,
    pub starts: String, // HH:MM or "Hoje HH:MM"
    pub is_meet: bool,
    pub meet_room_id: Option<String>,
    /// The caller's RSVP status: "ACCEPTED"/"DECLINED"/"TENTATIVE"/
    /// "NEEDS-ACTION", or "" when the caller isn't an attendee (their own
    /// event) — drives the quick-RSVP buttons on the home agenda.
    pub my_partstat: String,
}

impl HomeEvent {
    /// PT label for the current RSVP status, or "" when not an attendee.
    pub fn rsvp_label(&self) -> &'static str {
        match self.my_partstat.as_str() {
            "ACCEPTED" => "✓ Confirmado",
            "DECLINED" => "✗ Recusado",
            "TENTATIVE" => "? Talvez",
            "NEEDS-ACTION" => "Aguardando resposta",
            _ => "",
        }
    }

    /// Whether to show the quick-RSVP buttons (the caller is an invited
    /// attendee who can still change their answer).
    pub fn can_rsvp(&self) -> bool {
        !self.my_partstat.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct HomeDriveFile {
    pub id: String,
    pub name: String,
    pub kind: String,
}
impl HomeDriveFile {
    pub fn icon(&self) -> &'static str {
        if self.kind == "folder" {
            "📁"
        } else {
            "📄"
        }
    }
}

#[derive(Template)]
#[template(path = "home.html")]
pub struct HomeTpl {
    pub me: Me,
    pub mail_unread: i64,
    pub inbox_id: String,
    pub events: Vec<HomeEvent>,
    pub drive_files: Vec<HomeDriveFile>,
    pub chat_unread: i64,
    /// Pending tasks due today or earlier (max 8), for the home widget.
    pub tasks_due: Vec<TaskRow>,
    /// Default calendar id — target for the home quick-add-task form (empty
    /// when the user has no calendar; the quick-add then hides).
    pub tasks_cal_id: String,
    /// Upcoming calendar reminders within 24h (max 6), for the home widget.
    pub reminders: Vec<HomeReminder>,
    /// Contacts with a birthday in the next 30 days (max 5), soonest first.
    pub birthdays: Vec<HomeBirthday>,
}

/// One upcoming contact birthday for the home widget.
pub struct HomeBirthday {
    pub name: String,
    /// "DD/MM" of the birthday.
    pub when: String,
    /// Days until it (0 = today).
    pub days: i64,
}

/// One upcoming calendar alarm shown in the home reminders widget.
pub struct HomeReminder {
    /// Reminder text (VALARM description) or a fallback.
    pub text: String,
    /// "YYYY-MM-DD HH:MM" trigger time.
    pub when: String,
}

// ─── Calendar events ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct Event {
    pub id: String,
    pub calendar_id: String,
    pub uid: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub dtstart: Option<String>,
    #[serde(default)]
    pub dtend: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub organizer_email: Option<String>,
    #[serde(default)]
    pub rrule: Option<String>,
    #[serde(default)]
    pub ical_raw: Option<String>,
}

impl Event {
    pub fn title(&self) -> &str {
        self.summary.as_deref().unwrap_or("(sem título)")
    }
    pub fn is_recurring(&self) -> bool {
        self.rrule.is_some()
    }
    /// True when dtstart is a bare date (YYYY-MM-DD, no 'T').
    pub fn is_all_day(&self) -> bool {
        self.dtstart
            .as_deref()
            .map(|s| !s.contains('T'))
            .unwrap_or(false)
    }
    /// HH:MM slice from RFC3339 dtstart → fallback "".
    pub fn time_label(&self) -> String {
        let Some(s) = &self.dtstart else {
            return String::new();
        };
        // s like "2026-05-01T10:00:00+00:00" — take chars 11..16
        if s.len() >= 16 {
            s[11..16].to_string()
        } else {
            String::new()
        }
    }
    pub fn date_key(&self) -> String {
        self.dtstart
            .as_deref()
            .map(|s| s.get(0..10).unwrap_or("").to_string())
            .unwrap_or_default()
    }
}

/// One cell of the month grid.
#[derive(Debug, Clone)]
pub struct MonthCell {
    pub iso: String, // YYYY-MM-DD
    pub day: u8,
    pub in_month: bool,
    pub is_today: bool,
    pub events: Vec<Event>,
}

#[derive(Template)]
#[template(path = "calendar_month.html")]
pub struct CalendarMonthTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    pub selected: Calendar,
    pub year: i32,
    pub month: u8,
    pub month_label: String,
    pub prev_link: String,
    pub next_link: String,
    pub today_link: String,
    pub weekday_labels: Vec<&'static str>,
    pub weeks: Vec<Vec<MonthCell>>,
}

/// One column in week/day view.
#[derive(Debug, Clone)]
pub struct DayColumn {
    pub date_iso: String, // YYYY-MM-DD
    pub label: String,    // "Seg 01/05"
    pub is_today: bool,
    pub events: Vec<Event>,
}

impl DayColumn {
    /// All events are all-day (or there are none) — askama can't run closures.
    pub fn all_all_day(&self) -> bool {
        self.events.iter().all(|e| e.is_all_day())
    }
}

#[derive(Template)]
#[template(path = "calendar_week.html")]
pub struct CalendarWeekTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    pub selected: Calendar,
    pub week_label: String,
    pub prev_link: String,
    pub next_link: String,
    pub today_link: String,
    pub month_link: String,
    pub day_link: String,
    pub days: Vec<DayColumn>, // 7
}

#[derive(Template)]
#[template(path = "calendar_day.html")]
pub struct CalendarDayTpl {
    pub me: Me,
    pub calendars: Vec<Calendar>,
    pub selected: Calendar,
    pub date_label: String,
    pub date_iso: String,
    pub prev_link: String,
    pub next_link: String,
    pub today_link: String,
    pub week_link: String,
    pub month_link: String,
    pub events: Vec<Event>,
    pub hours: Vec<u8>,
}

impl CalendarDayTpl {
    /// Any all-day events present — askama can't run closures in `{% if %}`.
    pub fn has_all_day_events(&self) -> bool {
        self.events.iter().any(|e| e.is_all_day())
    }

    /// Zero-padded two-digit hour ("09"). askama rejects both the `*h` deref and
    /// the `&u8`-vs-int comparison the template would otherwise need.
    pub fn hh2(&self, h: &u8) -> String {
        format!("{h:02}")
    }
}

#[derive(Template)]
#[template(path = "event_print.html")]
pub struct EventPrintTpl {
    pub me: Me,
    pub calendar_name: String,
    pub event: Event,
}

#[derive(Template)]
#[template(path = "event_form.html")]
pub struct EventFormTpl {
    pub me: Me,
    pub calendar: Calendar,
    pub event_id: Option<String>,
    pub summary: String,
    pub location: String,
    pub description: String,
    pub dtstart: String, // datetime-local value "YYYY-MM-DDTHH:MM"
    pub dtend: String,
    pub attendees: String, // one email per line / comma-separated
    pub attendee_pills: Vec<AttendeePill>,
    /// Reminder lead times in minutes, comma-separated (seeds the form's
    /// reminder rows). Empty on a new event.
    pub reminders: String,
    /// Comma-separated categories, seeding the form's categories input.
    pub categories: String,
    /// The tenant's bookable resources, offered as checkboxes.
    pub resources: Vec<Resource>,
    /// Emails of resources already booked on this event (seeds the checkboxes on
    /// edit). Empty on a new event.
    pub booked_resources: Vec<String>,
    /// Newline-separated attachment URLs (ATTACH), seeding the form. Empty on
    /// a new event.
    pub attachments: String,
    pub error: Option<String>,
}

impl EventFormTpl {
    /// Whether a resource email is already booked on this event (case-insensitive).
    pub fn is_booked(&self, email: &str) -> bool {
        self.booked_resources
            .iter()
            .any(|b| b.eq_ignore_ascii_case(email))
    }
}

#[derive(Debug, Clone)]
pub struct AttendeePill {
    pub email: String,
    pub partstat: String, // raw uppercase: NEEDS-ACTION | ACCEPTED | DECLINED | TENTATIVE
}

impl AttendeePill {
    pub fn label(&self) -> &'static str {
        match self.partstat.as_str() {
            "ACCEPTED" => "aceito",
            "DECLINED" => "recusado",
            "TENTATIVE" => "talvez",
            _ => "pendente",
        }
    }
    pub fn css(&self) -> &'static str {
        match self.partstat.as_str() {
            "ACCEPTED" => "ok",
            "DECLINED" => "off",
            "TENTATIVE" => "warn",
            _ => "muted",
        }
    }
}

/// One EMAIL entry parsed from a contact's vCard (with its TYPE label).
#[derive(Deserialize)]
pub struct ContactEmailRow {
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub label: Option<String>,
}
impl ContactEmailRow {
    pub fn label_text(&self) -> &str {
        self.label
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("—")
    }
}

/// One structured ADDRESS entry from a contact's vCard.
#[derive(Deserialize)]
pub struct ContactAddressRow {
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub street: Option<String>,
    #[serde(default)]
    pub locality: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub postal_code: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}
impl ContactAddressRow {
    pub fn label_text(&self) -> &str {
        self.label
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("—")
    }
    /// One-line "street, locality region postal, country" with blanks skipped.
    pub fn one_line(&self) -> String {
        let parts = [
            self.street.as_deref(),
            self.locality.as_deref(),
            self.region.as_deref(),
            self.postal_code.as_deref(),
            self.country.as_deref(),
        ];
        parts
            .iter()
            .filter_map(|p| p.map(str::trim).filter(|s| !s.is_empty()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Template)]
#[template(path = "contact_form.html")]
pub struct ContactFormTpl {
    pub me: Me,
    pub book: AddressBook,
    pub contact_id: Option<String>,
    pub full_name: String,
    pub given_name: String,
    pub family_name: String,
    pub email: String,
    pub phone: String,
    pub organization: String,
    pub error: Option<String>,
    /// All EMAIL/ADDRESS entries from the vCard (edit view only; empty on create).
    pub emails: Vec<ContactEmailRow>,
    pub addresses: Vec<ContactAddressRow>,
}

// ─── ACL share templates ────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct AclRow {
    #[serde(alias = "grantee_id")]
    pub grantee_id: String,
    pub privilege: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Template)]
#[template(path = "calendar_share.html")]
pub struct CalendarShareTpl {
    pub me: Me,
    pub calendar: Calendar,
    pub shares: Vec<AclRow>,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "addrbook_share.html")]
pub struct AddrbookShareTpl {
    pub me: Me,
    pub addressbook: AddressBook,
    pub shares: Vec<AclRow>,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "drive_acl.html")]
pub struct DriveAclTpl {
    pub me: Me,
    /// File/folder id and display name being shared.
    pub file_id: String,
    pub file_name: String,
    pub shares: Vec<AclRow>,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "note_acl.html")]
pub struct NoteAclTpl {
    pub me: Me,
    pub note_id: String,
    pub note_title: String,
    pub shares: Vec<AclRow>,
    pub error: Option<String>,
}

/// Mail attachment metadata (from backend /attachments list).
#[derive(Debug, Deserialize, Clone)]
pub struct Attachment {
    pub index: u32,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size: i64,
}
impl Attachment {
    pub fn name(&self) -> &str {
        self.filename.as_deref().unwrap_or("anexo")
    }
    pub fn size_human(&self) -> String {
        let b = self.size as f64;
        if b < 1024.0 {
            format!("{} B", self.size)
        } else if b < 1_048_576.0 {
            format!("{:.1} KB", b / 1024.0)
        } else {
            format!("{:.1} MB", b / 1_048_576.0)
        }
    }
}

/// GAL contact (from /api/v1/gal/search).
#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct GalContact {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub department: Option<String>,
}
impl GalContact {
    pub fn display(&self) -> &str {
        self.display_name.as_deref().unwrap_or("")
    }
    pub fn email_str(&self) -> &str {
        self.email.as_deref().unwrap_or("")
    }
}

/// Meeting room (list item from /api/v1/meetings).
#[derive(Debug, Deserialize, Clone)]
pub struct MeetRoom {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub scheduled_at: Option<String>,
    #[serde(default)]
    pub scheduled_end: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub recording_url: Option<String>,
    #[serde(default)]
    pub recording_started_at: Option<String>,
    #[serde(default)]
    pub participant_count: i64,
    #[serde(default)]
    pub duration_minutes: Option<i64>,
}
impl MeetRoom {
    pub fn title(&self) -> &str {
        self.name.as_deref().unwrap_or("Reunião")
    }
    /// True when the backend has an active server-side recording session.
    pub fn is_recording(&self) -> bool {
        self.recording_started_at.is_some()
    }
    pub fn room_id_short(&self) -> &str {
        &self.id[..self.id.len().min(8)]
    }
    pub fn is_scheduled(&self) -> bool {
        self.scheduled_at.is_some()
    }
    pub fn is_ended(&self) -> bool {
        self.status.as_deref() == Some("ended")
    }
    pub fn scheduled_time(&self) -> String {
        let Some(s) = &self.scheduled_at else {
            return String::new();
        };
        // "2026-05-23T14:00:00+00:00" → "23/05 14:00"
        if s.len() >= 16 {
            format!("{}/{} {}:{}", &s[8..10], &s[5..7], &s[11..13], &s[14..16])
        } else {
            s.clone()
        }
    }
}

// ─── Chat ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct ChatChannel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub unread_count: i64,
}

impl ChatChannel {
    pub fn icon(&self) -> &'static str {
        match self.kind.as_deref() {
            Some("direct") => "@",
            _ => "#",
        }
    }
    pub fn is_direct(&self) -> bool {
        self.kind.as_deref() == Some("direct")
    }
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct ChatMessage {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub body: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub edited: bool,
    #[serde(default)]
    pub parent_id: Option<String>,
}

impl ChatMessage {
    pub fn sender(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.user_id)
    }
    pub fn time_label(&self) -> String {
        let Some(s) = &self.created_at else {
            return String::new();
        };
        if s.len() >= 16 {
            s[11..16].to_string()
        } else {
            String::new()
        }
    }
    pub fn is_own(&self, me_id: &str) -> bool {
        self.user_id == me_id
    }
}

/// One file shared in a channel (the "📎 Arquivos" panel). Bytes are fetched
/// through the chat download proxy by `id`.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatAttachment {
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub content_type: String,
    #[serde(default)]
    pub size_bytes: i64,
    #[serde(default)]
    pub kind: String,
}

impl ChatAttachment {
    pub fn size_human(&self) -> String {
        let b = self.size_bytes as f64;
        if self.size_bytes < 1024 {
            format!("{} B", self.size_bytes)
        } else if b < 1024.0 * 1024.0 {
            format!("{:.1} KB", b / 1024.0)
        } else {
            format!("{:.1} MB", b / (1024.0 * 1024.0))
        }
    }
    pub fn icon(&self) -> &'static str {
        match self.kind.as_str() {
            "image" => "🖼",
            "video" => "🎬",
            "audio" => "🎵",
            _ => "📄",
        }
    }
}

#[derive(Template)]
#[template(path = "chat.html")]
pub struct ChatTpl {
    pub me: Me,
    pub channels: Vec<ChatChannel>,
    pub active_channel: Option<ChatChannel>,
    pub messages: Vec<ChatMessage>,
    pub attachments: Vec<ChatAttachment>,
}

impl ChatTpl {
    /// Whether the user has any direct-message channels — askama can't run the
    /// closure `channels.iter().any(|c| c.is_direct())` inline.
    pub fn has_dms(&self) -> bool {
        self.channels.iter().any(|c| c.is_direct())
    }

    /// True when the unread divider ("N new messages") belongs *before* the
    /// 1-based message position `idx` (askama's `loop.index`). Computed here
    /// because askama can't parse the `as usize` cast inline.
    pub fn is_unread_divider(&self, idx: &usize) -> bool {
        let unread = self
            .active_channel
            .as_ref()
            .map(|c| c.unread_count)
            .unwrap_or(0);
        if unread <= 0 {
            return false;
        }
        *idx == self.messages.len().saturating_sub(unread as usize)
    }
}

#[derive(Template)]
#[template(path = "meet.html")]
pub struct MeetTpl {
    pub me: Me,
    pub meetings: Vec<MeetRoom>,
    pub upcoming: Vec<MeetRoom>,
    pub past: Vec<MeetRoom>,
    pub flash: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MeetParticipant {
    pub user_id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
}
impl MeetParticipant {
    pub fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.user_id)
    }
    pub fn initial(&self) -> char {
        self.name().chars().next().unwrap_or('?')
    }
}

#[derive(Template)]
#[template(path = "meet_room.html")]
pub struct MeetRoomTpl {
    pub me: Me,
    pub room_id: String,
    pub room_name: String,
    pub meeting: Option<MeetRoom>,
    pub participants: Vec<MeetParticipant>,
    pub jitsi_domain: String,
    pub jitsi_jwt: String,
    pub jitsi_enabled: bool,
    pub join_only: bool,
    pub is_moderator: bool,
}

// ─── Tasks ───────────────────────────────────────────────────────────────────

/// One server-backed VTODO task row (subset of the backend Task for display).
#[derive(Debug, Clone, Deserialize)]
pub struct TaskRow {
    pub id: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub priority: i16,
    #[serde(default)]
    pub due: Option<String>,
    /// RFC 5545 recurrence rule, when the task repeats.
    #[serde(default)]
    pub rrule: Option<String>,
}

impl TaskRow {
    pub fn is_done(&self) -> bool {
        self.status == "COMPLETED" || self.status == "CANCELLED"
    }

    /// Compact PT label for the recurrence badge ("" when one-off).
    pub fn repeat_label(&self) -> &'static str {
        match self.rrule.as_deref() {
            Some(r) if r.contains("FREQ=DAILY") => "diária",
            Some(r) if r.contains("FREQ=WEEKLY") => "semanal",
            Some(r) if r.contains("FREQ=MONTHLY") => "mensal",
            Some(_) => "recorrente",
            None => "",
        }
    }

    /// Form value matching the recurrence (for the inline edit select):
    /// "daily"/"weekly"/"monthly", or "" for one-off / unsupported rules.
    pub fn repeat_value(&self) -> &'static str {
        match self.rrule.as_deref() {
            Some(r) if r.contains("FREQ=DAILY") => "daily",
            Some(r) if r.contains("FREQ=WEEKLY") => "weekly",
            Some(r) if r.contains("FREQ=MONTHLY") => "monthly",
            _ => "",
        }
    }

    /// Date portion of the RFC3339 due, for compact display ("YYYY-MM-DD").
    pub fn due_date(&self) -> &str {
        match &self.due {
            Some(d) if d.len() >= 10 => &d[..10],
            _ => "",
        }
    }
    pub fn priority_label(&self) -> &'static str {
        match self.priority {
            1..=4 => "Alta",
            5 => "Média",
            6..=9 => "Baixa",
            _ => "",
        }
    }
}

#[derive(Template)]
#[template(path = "tasks.html")]
pub struct TasksTpl {
    pub me: Me,
    pub tasks: Vec<TaskRow>,
    /// The calendar tasks are stored in (the user's default). Empty when the user
    /// has no calendar yet — the page shows a hint instead of the form.
    pub cal_id: String,
}

#[derive(Template)]
#[template(path = "meet_schedule.html")]
pub struct MeetScheduleTpl {
    pub me: Me,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "mail_search.html")]
pub struct MailSearchTpl {
    pub me: Me,
    pub folders: Vec<Folder>,
    pub messages: Vec<MessageListItem>,
    pub query: String,
    pub search_from: String,
    pub search_folder: String,
    pub search_date_from: String,
    pub search_date_to: String,
    pub search_has_attachment: bool,
}

#[derive(Template)]
#[template(path = "gal_search.html")]
pub struct GalSearchTpl {
    pub me: Me,
    pub contacts: Vec<GalContact>,
    pub query: String,
}

#[derive(Template)]
#[template(path = "settings.html")]
pub struct SettingsTpl {
    pub me: Me,
    pub tab: String,
    pub flash: Option<String>,
    pub logout_url: String,
    pub kc_account: String,
    pub signature_enabled: bool,
    pub signature_body: Option<String>,
    pub autoreply_enabled: bool,
    pub autoreply_subject: Option<String>,
    pub autoreply_body: Option<String>,
    pub autoreply_start: Option<String>,
    pub autoreply_end: Option<String>,
    pub sieve_script: Option<String>,
    pub sieve_error: Option<String>,
    pub aliases: Vec<MailAlias>,
    /// User's flag presets (loaded only on the flag_presets tab).
    pub flag_presets: Vec<FlagPreset>,
    /// One row per weekday (Mon..Sun), with the configured window as HH:MM
    /// strings (empty when that day is off).
    pub working_days: Vec<WorkingDayRow>,
    /// Server-backed notification toggles (default true when unset).
    pub notify_new_mail: bool,
    pub notify_flags_changed: bool,
    pub notify_folder_updated: bool,
}

/// A weekday row for the working-hours editor (times pre-formatted as HH:MM).
pub struct WorkingDayRow {
    /// Backend weekday index 0..6.
    pub weekday: i16,
    pub label: String,
    pub enabled: bool,
    pub start: String,
    pub end: String,
}

/// A working-hours window as returned by the calendar backend.
#[derive(Debug, Deserialize, Clone)]
pub struct WorkingHour {
    pub weekday: i16,
    pub start_minute: i32,
    pub end_minute: i32,
}

/// A tenant email alias (`alias -> target` forwarding) for the settings screen.
#[derive(Debug, Deserialize, Clone)]
pub struct MailAlias {
    pub id: String,
    pub alias: String,
    pub target: String,
    #[serde(default)]
    pub is_enabled: bool,
}

/// A named set of IMAP flags for quick-apply (mail settings → flag presets).
#[derive(Debug, Clone, Deserialize)]
pub struct FlagPreset {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub flags: Vec<String>,
}

impl FlagPreset {
    pub fn flags_csv(&self) -> String {
        self.flags.join(", ")
    }
}

/// A mailbox delegation grant as deserialized from the mail backend (ids).
#[derive(Debug, Deserialize, Clone)]
pub struct DelegationRaw {
    pub id: String,
    pub owner_id: String,
    pub delegate_id: String,
    pub access: String,
}

/// A delegation row for the screen, with the counterparty id resolved to an
/// email for display.
pub struct DelegationView {
    pub id: String,
    /// The other party's email (delegate when listing given, owner when given-to-me).
    pub who: String,
    pub access: String,
    /// The counterparty's user id (owner, for "to-me" rows → open-mailbox link).
    pub who_id: String,
}

#[derive(Template)]
#[template(path = "delegations.html")]
pub struct DelegationsTpl {
    pub me: Me,
    pub flash: Option<String>,
    /// Grants the caller has given (delegate's email shown).
    pub granted: Vec<DelegationView>,
    /// Grants given to the caller (owner's email shown).
    pub to_me: Vec<DelegationView>,
}

/// One email signature in the multi-signature manager.
pub struct SignatureRow {
    pub id: String,
    pub name: String,
    pub content: String,
    /// "html" or "plain".
    pub format: String,
    pub is_default: bool,
}

#[derive(Template)]
#[template(path = "settings_signatures.html")]
pub struct SettingsSignaturesTpl {
    pub me: Me,
    pub rows: Vec<SignatureRow>,
    pub flash: Option<String>,
}

/// One message template (canned response) in the manager.
pub struct MessageTemplateRow {
    pub id: String,
    pub name: String,
    pub subject: String,
    pub body: String,
}

#[derive(Template)]
#[template(path = "settings_templates.html")]
pub struct SettingsTemplatesTpl {
    pub me: Me,
    pub rows: Vec<MessageTemplateRow>,
    pub flash: Option<String>,
}

#[derive(Template)]
#[template(path = "settings_blocked_senders.html")]
pub struct SettingsBlockedSendersTpl {
    pub me: Me,
    /// Blocked email addresses, sorted.
    pub addresses: Vec<String>,
    /// Safe (allow-listed) email addresses, sorted.
    pub safe_addresses: Vec<String>,
    pub flash: Option<String>,
}

/// One personal access token (metadata only — the secret is never listed).
pub struct ApiTokenRow {
    pub id: String,
    pub name: String,
    /// "YYYY-MM-DD HH:MM"
    pub created: String,
    /// Last use, or empty if never used.
    pub last_used: String,
    /// Expiry, or empty for non-expiring tokens.
    pub expires: String,
    /// False once revoked.
    pub active: bool,
}

#[derive(Template)]
#[template(path = "settings_tokens.html")]
pub struct SettingsTokensTpl {
    pub me: Me,
    pub rows: Vec<ApiTokenRow>,
    /// Cleartext of a token minted by THIS request — shown exactly once.
    pub new_token: Option<String>,
    pub flash: Option<String>,
}

// ─── Admin panel ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct AdminUser {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tenant_id: String,
    #[serde(default)]
    pub created_at: Option<String>,
}
impl AdminUser {
    pub fn initial(&self) -> char {
        let name = self.display_name.as_deref().unwrap_or(&self.email);
        name.chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .next()
            .unwrap_or('?')
    }
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct AdminTenant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub user_count: i64,
    #[serde(default)]
    pub quota_gb: Option<i64>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize)]
pub struct AuditEvent {
    pub ts: String,
    pub user_email: String,
    pub action: String,
    #[serde(default)]
    pub resource_id: Option<String>,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AdminConfig {
    pub platform_name: String,
    pub logo_url: String,
    pub accent_color: String,
    pub mail_domain: String,
    pub mail_quota_mb: i64,
    pub allow_external_relay: bool,
    pub jitsi_domain: String,
    pub jitsi_recording: bool,
    pub drive_quota_gb: i64,
    pub blocked_extensions: String,
    pub require_mfa: bool,
    pub session_hours: i64,
    pub allowed_cidrs: String,
}

#[derive(Template)]
#[template(path = "admin_users.html")]
pub struct AdminUsersTpl {
    pub me: Me,
    pub users: Vec<AdminUser>,
    pub flash: Option<String>,
}

#[derive(Template)]
#[template(path = "admin_tenants.html")]
pub struct AdminTenantsTpl {
    pub me: Me,
    pub tenants: Vec<AdminTenant>,
    pub flash: Option<String>,
}

#[derive(Template)]
#[template(path = "admin_monitoring.html")]
pub struct AdminMonitoringTpl {
    pub me: Me,
}

#[derive(Template)]
#[template(path = "admin_audit.html")]
pub struct AdminAuditTpl {
    pub me: Me,
    pub events: Vec<AuditEvent>,
}

#[derive(Template)]
#[template(path = "admin_config.html")]
pub struct AdminConfigTpl {
    pub me: Me,
    pub config: AdminConfig,
    pub flash: Option<String>,
}

/// A bookable calendar resource (meeting room / equipment).
#[derive(Debug, Clone, Deserialize)]
pub struct Resource {
    pub id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub capacity: Option<i32>,
    #[serde(default)]
    pub is_active: bool,
}

#[derive(Template)]
#[template(path = "admin_resources.html")]
pub struct AdminResourcesTpl {
    pub me: Me,
    pub resources: Vec<Resource>,
    pub flash: Option<String>,
}

/// One dead-lettered notification (a webhook that exhausted its retries).
#[derive(Debug, Clone)]
pub struct DlqEntry {
    pub id: String,
    pub tenant_id: String,
    pub user_id: String,
    pub kind: String,
    pub attempts: i64,
    pub last_error: String,
    pub failed_at: String,
    /// One-line JSON preview of the saved payload (truncated for the table).
    pub payload_preview: String,
}

/// A `(kind, count)` tally from the DLQ stats endpoint.
#[derive(Debug, Clone)]
pub struct DlqKindCount {
    pub kind: String,
    pub count: i64,
}

#[derive(Template)]
#[template(path = "admin_dlq.html")]
pub struct AdminDlqTpl {
    pub me: Me,
    pub total: i64,
    pub entries: Vec<DlqEntry>,
    pub by_kind: Vec<DlqKindCount>,
    /// Active `kind` filter, echoed back into the filter input.
    pub filter_kind: String,
    pub flash: Option<String>,
}

/// One folder retention policy in the admin retention page.
pub struct RetentionPolicyRow {
    pub id: String,
    /// Folder name, or empty for "all folders".
    pub folder: String,
    pub retain_days: i64,
    pub action: String,
    pub enabled: bool,
}

#[derive(Template)]
#[template(path = "admin_retention.html")]
pub struct AdminRetentionTpl {
    pub me: Me,
    /// Tenant-wide default archive retention in days (backend default 365).
    pub default_days: i64,
    pub policies: Vec<RetentionPolicyRow>,
    pub flash: Option<String>,
}

#[derive(Debug, Deserialize, Clone, serde::Serialize, Default)]
pub struct AdminLoginEvent {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub ip: Option<String>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub success: bool,
}

/// One metric line in the per-tenant usage report.
pub struct TenantUsageRow {
    pub label: String,
    /// Pre-formatted value (count or human-readable size).
    pub value: String,
}

#[derive(Template)]
#[template(path = "admin_tenant_usage.html")]
pub struct AdminTenantUsageTpl {
    pub me: Me,
    pub tenant_id: String,
    pub rows: Vec<TenantUsageRow>,
}

/// One MFA credential of a user (the admin MFA panel).
pub struct MfaFactorRow {
    pub id: String,
    /// Keycloak credential type: "otp" / "webauthn" / "webauthn-passwordless".
    pub kind: String,
    /// User-given device label, or empty.
    pub label: String,
    /// "YYYY-MM-DD HH:MM" or empty.
    pub created: String,
}

#[derive(Template)]
#[template(path = "admin_user_detail.html")]
pub struct AdminUserDetailTpl {
    pub me: Me,
    pub user: AdminUser,
    pub logins: Vec<AdminLoginEvent>,
    /// MFA factors (superadmin only). None = unavailable (not superadmin,
    /// or the auth service has no Keycloak admin client configured).
    pub mfa: Option<Vec<MfaFactorRow>>,
    pub flash: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_size_bytes() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1023), "1023 B");
    }

    #[test]
    fn human_size_kilobytes() {
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(2048), "2.0 KB");
    }

    #[test]
    fn human_size_megabytes() {
        assert_eq!(human_size(1_048_576), "1.0 MB");
        assert_eq!(human_size(5 * 1_048_576), "5.0 MB");
    }

    #[test]
    fn human_size_gigabytes() {
        assert_eq!(human_size(1_073_741_824), "1.00 GB");
        assert_eq!(human_size(2 * 1_073_741_824), "2.00 GB");
    }

    #[test]
    fn human_size_boundary_1023_bytes() {
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
    }

    #[test]
    fn human_size_negative_treated_as_zero_or_shown() {
        // negative sizes shouldn't panic — just verify it doesn't
        let _ = human_size(-1);
    }

    #[test]
    fn drive_file_lock_state_and_holder() {
        let mut f: DriveFile = serde_json::from_value(serde_json::json!({
            "id": "f1", "name": "doc.txt", "kind": "file"
        }))
        .expect("parse");
        assert!(!f.is_locked());
        assert!(!f.locked_by_me("u1"));
        f.locked_at = Some("2026-06-03T10:00:00Z".into());
        f.locked_by = Some("u1".into());
        assert!(f.is_locked());
        assert!(f.locked_by_me("u1"));
        assert!(!f.locked_by_me("u2"));
    }

    #[test]
    fn contact_address_one_line_skips_blanks() {
        let a: ContactAddressRow = serde_json::from_value(serde_json::json!({
            "label": "HOME", "street": "Rua A 10", "locality": "Curitiba",
            "region": "", "postal_code": "80000-000", "country": "Brasil"
        }))
        .expect("parse");
        assert_eq!(a.label_text(), "HOME");
        assert_eq!(a.one_line(), "Rua A 10, Curitiba, 80000-000, Brasil");
        let e: ContactEmailRow =
            serde_json::from_value(serde_json::json!({ "address": "x@y.com" })).expect("parse");
        assert_eq!(e.label_text(), "—");
    }

    #[test]
    fn drive_file_expiry_human_truncates() {
        let mut f: DriveFile = serde_json::from_value(serde_json::json!({
            "id": "f1", "name": "doc.txt", "kind": "file"
        }))
        .expect("parse");
        assert!(!f.has_expiry());
        assert_eq!(f.expiry_human(), "");
        f.expires_at = Some("2026-07-01T00:00:00Z".into());
        assert!(f.has_expiry());
        assert_eq!(f.expiry_human(), "2026-07-01 00:00");
    }

    #[test]
    fn shared_note_row_privilege_and_label() {
        let r: SharedNoteRow = serde_json::from_value(serde_json::json!({
            "id": "n1", "title": "Roadmap", "privilege": "write",
            "updated_at": "2026-06-02T13:45:00Z"
        }))
        .expect("parse");
        assert_eq!(r.label(), "Roadmap");
        assert_eq!(r.privilege_label(), "Edição");
        assert_eq!(r.when(), "2026-06-02 13:45");
        let ro: SharedNoteRow =
            serde_json::from_value(serde_json::json!({ "id": "n2", "privilege": "read" }))
                .expect("parse");
        assert_eq!(ro.label(), "(sem título)");
        assert_eq!(ro.privilege_label(), "Leitura");
    }

    #[test]
    fn note_version_row_label_falls_back_when_blank() {
        let v: NoteVersionRow = serde_json::from_value(serde_json::json!({
            "version_no": 3, "title": "Plano Q3", "created_at": "2026-06-01T08:00:00Z"
        }))
        .expect("parse");
        assert_eq!(v.version_no, 3);
        assert_eq!(v.label(), "Plano Q3");
        let blank: NoteVersionRow =
            serde_json::from_value(serde_json::json!({ "version_no": 1, "title": "  " }))
                .expect("parse");
        assert_eq!(blank.label(), "(sem título)");
    }

    #[test]
    fn meet_room_is_recording_reflects_started_at() {
        let mut m: MeetRoom =
            serde_json::from_value(serde_json::json!({ "id": "abcd1234" })).expect("parse");
        assert!(!m.is_recording());
        m.recording_started_at = Some("2026-06-03T10:00:00Z".into());
        assert!(m.is_recording());
    }

    #[test]
    fn human_size_exact_mb_boundary() {
        assert_eq!(human_size(1024 * 1024 - 1), "1024.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn human_size_large_gb_value() {
        assert_eq!(human_size(10 * 1_073_741_824), "10.00 GB");
    }

    #[test]
    fn human_size_zero_bytes() {
        assert_eq!(human_size(0), "0 B");
    }

    #[test]
    fn human_size_terabytes_scale() {
        let tb = 1_099_511_627_776i64; // 1 TiB
        let s = human_size(tb);
        assert!(s.ends_with("GB") || s.ends_with("TB") || s.contains('.'));
    }

    #[test]
    fn human_size_one_byte() {
        let s = human_size(1);
        assert!(!s.is_empty());
    }

    #[test]
    fn human_size_zero_is_non_empty() {
        let s = human_size(0);
        assert!(!s.is_empty());
    }

    #[test]
    fn human_size_negative_is_non_empty() {
        let s = human_size(-1);
        assert!(!s.is_empty());
    }

    #[test]
    fn human_size_zero_produces_string() {
        let s = human_size(0);
        assert!(!s.is_empty());
    }

    #[test]
    fn human_size_512_bytes_displayed() {
        let s = human_size(512);
        assert!(s.contains("512") && s.contains("B"));
    }

    #[test]
    fn human_size_two_kibibytes_is_two_kb() {
        assert_eq!(human_size(2 * 1024), "2.0 KB");
    }

    #[test]
    fn human_size_one_kibibyte_is_1_kb() {
        assert_eq!(human_size(1024), "1.0 KB");
    }

    #[test]
    fn human_size_four_kibibytes_is_4_kb() {
        assert_eq!(human_size(4 * 1024), "4.0 KB");
    }

    #[test]
    fn human_size_1023_bytes_stays_below_kb() {
        let s = human_size(1023);
        assert!(s.ends_with('B') && !s.contains("KB"));
    }

    #[test]
    fn human_size_one_mebibyte_is_1_mb() {
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn human_size_negative_bytes_handled() {
        let s = human_size(-1);
        assert!(!s.is_empty());
    }

    #[test]
    fn human_size_max_i64_does_not_panic() {
        let s = human_size(i64::MAX);
        assert!(!s.is_empty());
    }

    #[test]
    fn human_size_two_gibibytes_is_two_gb() {
        assert_eq!(human_size(2 * 1_073_741_824), "2.00 GB");
    }
}
