//! Askama templates.
#![allow(dead_code)]

use askama::Template;
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct MfaInfo {
    #[serde(default)] pub totp:     bool,
    #[serde(default)] pub webauthn: bool,
    #[serde(default)] pub amr:      Vec<String>,
    #[serde(default)] pub acr:      Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Me {
    pub user_id:       String,
    pub tenant_id:     String,
    pub email:         String,
    #[serde(default)]  pub display_name:  Option<String>,
    #[serde(default)]  pub roles:         Vec<String>,
    #[serde(default)]  pub expires_at:    i64,
    #[serde(default)]  pub mfa:           Option<MfaInfo>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Folder {
    pub id:            String,
    pub name:          String,
    #[serde(default)]  pub special_use:    Option<String>,
    #[serde(default)]  pub message_count:  i64,
    #[serde(default)]  pub unseen_count:   i64,
}



impl Folder {
    pub fn icon(&self) -> &'static str {
        match self.special_use.as_deref() {
            Some("\\Inbox")  => "📥",
            Some("\\Sent")   => "📤",
            Some("\\Drafts") => "📝",
            Some("\\Trash")  => "🗑",
            Some("\\Junk")   => "🚫",
            _ => "📁",
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct MessageListItem {
    pub id:              String,
    #[serde(default)] pub subject:         Option<String>,
    #[serde(default)] pub from_addr:       Option<String>,
    #[serde(default)] pub from_name:       Option<String>,
    #[serde(default)] pub preview_text:    Option<String>,
    #[serde(default)] pub flags:           Vec<String>,
    #[serde(default)] pub date:            Option<String>,
    #[serde(default)] pub has_attachments: bool,
}

impl MessageListItem {
    pub fn is_unread(&self) -> bool { !self.flags.iter().any(|f| f == "\\Seen") }
    pub fn from_display(&self) -> &str {
        self.from_name.as_deref()
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

#[derive(Debug, Deserialize, Clone)]
pub struct MessageDetail {
    pub id:          String,
    #[serde(default)] pub subject:     Option<String>,
    #[serde(default)] pub from_addr:   Option<String>,
    #[serde(default)] pub from_name:   Option<String>,
    #[serde(default)] pub to_addrs:    serde_json::Value,
    #[serde(default)] pub cc_addrs:    serde_json::Value,
    #[serde(default)] pub date:        Option<String>,
    #[serde(default)] pub body_text:   Option<String>,
    #[serde(default)] pub body_html:   Option<String>,
    #[serde(default)] pub has_attachments: bool,
}

// ───── Templates ─────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "login.html")]
pub struct LoginTpl {
    pub login_url: String,
    pub error:     Option<String>,
}

#[derive(Template)]
#[template(path = "me.html")]
pub struct MeTpl {
    pub me:         Me,
    pub logout_url: String,
}

#[derive(Template)]
#[template(path = "security.html")]
pub struct SecurityTpl {
    pub me:         Me,
    pub kc_account: String,
}

#[derive(Template)]
#[template(path = "mail_list.html")]
pub struct MailListTpl {
    pub me:       Me,
    pub folders:  Vec<Folder>,
    pub selected: String,
    pub messages: Vec<MessageListItem>,
    pub detail:   Option<MessageDetail>,
    pub selected_id: Option<String>,
}

#[derive(Template)]
#[template(path = "mail_compose.html")]
pub struct MailComposeTpl {
    pub me:    Me,
    pub error: Option<String>,
}


// ─── Drive ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct DriveFile {
    pub id:         String,
    pub name:       String,
    pub kind:       String,
    #[serde(default)] pub size_bytes: i64,
    #[serde(default)] pub mime_type:  Option<String>,
    #[serde(default)] pub parent_id:  Option<String>,
    #[serde(default)] pub sha256:     Option<String>,
    #[serde(default)] pub created_at: Option<String>,
    #[serde(default)] pub deleted_at: Option<String>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool { self.kind == "folder" }
    pub fn size_human(&self) -> String {
        if self.is_folder() { return "—".into(); }
        let b = self.size_bytes as f64;
        if b < 1024.0 { format!("{} B", self.size_bytes) }
        else if b < 1024.0*1024.0 { format!("{:.1} KB", b/1024.0) }
        else if b < 1024.0*1024.0*1024.0 { format!("{:.1} MB", b/(1024.0*1024.0)) }
        else { format!("{:.1} GB", b/(1024.0*1024.0*1024.0)) }
    }
    pub fn icon(&self) -> &'static str {
        if self.is_folder() { "📁" } else { "📄" }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
pub struct DriveQuota {
    pub max_bytes:  i64,
    pub used_bytes: i64,
}

impl DriveQuota {
    pub fn percent(&self) -> i64 {
        if self.max_bytes == 0 { return 0; }
        (self.used_bytes * 100 / self.max_bytes).clamp(0, 100)
    }
    pub fn used_human(&self)  -> String { human_size(self.used_bytes) }
    pub fn max_human(&self)   -> String { human_size(self.max_bytes)  }
}

fn human_size(n: i64) -> String {
    let b = n as f64;
    if b < 1024.0              { format!("{b:.0} B")          }
    else if b < 1_048_576.0    { format!("{:.1} KB", b/1024.0)         }
    else if b < 1_073_741_824.0{ format!("{:.1} MB", b/1_048_576.0)    }
    else                       { format!("{:.2} GB", b/1_073_741_824.0)}
}

#[derive(Template)]
#[template(path = "drive.html")]
pub struct DriveTpl {
    pub me:         Me,
    pub parent_id:  Option<String>,
    pub files:      Vec<DriveFile>,
    pub quota:      Option<DriveQuota>,
}

#[derive(Template)]
#[template(path = "drive_trash.html")]
pub struct DriveTrashTpl {
    pub me:    Me,
    pub files: Vec<DriveFile>,
}

// ─── Calendar ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct Calendar {
    pub id:          String,
    pub name:        String,
    #[serde(default)] pub description: Option<String>,
    #[serde(default)] pub color:       Option<String>,
    #[serde(default)] pub is_default:  bool,
}

#[derive(Template)]
#[template(path = "calendar.html")]
pub struct CalendarTpl {
    pub me:        Me,
    pub calendars: Vec<Calendar>,
}

// ─── Contacts ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct AddressBook {
    pub id:          String,
    pub name:        String,
    #[serde(default)] pub description: Option<String>,
    #[serde(default)] pub is_default:  bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Contact {
    pub id:          String,
    #[serde(default)] pub full_name:    Option<String>,
    #[serde(default)] pub email:        Option<String>,
    #[serde(default)] pub phone:        Option<String>,
    #[serde(default)] pub organization: Option<String>,
}

impl Contact {
    pub fn name_display(&self) -> &str { self.full_name.as_deref().unwrap_or("—") }
    pub fn email_display(&self) -> &str { self.email.as_deref().unwrap_or("") }
    pub fn phone_display(&self) -> &str { self.phone.as_deref().unwrap_or("") }
    pub fn org_display(&self) -> &str { self.organization.as_deref().unwrap_or("") }
}

#[derive(Template)]
#[template(path = "contacts.html")]
pub struct ContactsTpl {
    pub me:            Me,
    pub books:         Vec<AddressBook>,
    pub selected_book: Option<String>,
    pub contacts:      Vec<Contact>,
}
