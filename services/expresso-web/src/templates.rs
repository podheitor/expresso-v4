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
}

#[derive(Template)]
#[template(path = "notes.html")]
pub struct NotesTpl {
    pub me: Me,
    pub notes: Vec<Note>,
    /// The note open in the editor pane (when one is selected), else None.
    pub selected: Option<Note>,
}

#[derive(Template)]
#[template(path = "search.html")]
pub struct SearchTpl {
    pub me: Me,
    pub query: String,
    pub groups: Vec<SearchGroup>,
    pub total: usize,
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
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.kind == "folder"
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
    pub error: Option<String>,
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
    pub participant_count: i64,
    #[serde(default)]
    pub duration_minutes: Option<i64>,
}
impl MeetRoom {
    pub fn title(&self) -> &str {
        self.name.as_deref().unwrap_or("Reunião")
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

#[derive(Template)]
#[template(path = "chat.html")]
pub struct ChatTpl {
    pub me: Me,
    pub channels: Vec<ChatChannel>,
    pub active_channel: Option<ChatChannel>,
    pub messages: Vec<ChatMessage>,
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

#[derive(Template)]
#[template(path = "tasks.html")]
pub struct TasksTpl {
    pub me: Me,
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

#[derive(Template)]
#[template(path = "admin_user_detail.html")]
pub struct AdminUserDetailTpl {
    pub me: Me,
    pub user: AdminUser,
    pub logins: Vec<AdminLoginEvent>,
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
