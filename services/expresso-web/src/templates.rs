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

