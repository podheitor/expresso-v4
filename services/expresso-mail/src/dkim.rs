//! Thin re-export wrapper — DKIM/SPF/DMARC logic lives in
//! `libs/expresso-mail-auth` (shared with expresso-milter).

pub use expresso_mail_auth::{AuthResults, DkimSignerState, verify_inbound};
