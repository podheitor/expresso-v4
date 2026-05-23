//! Fire-and-forget invite email for meeting participants.
//!
//! Reads MEET__SMTP_HOST (required), MEET__SMTP_PORT (default 587),
//! MEET__MAIL_FROM (default "noreply@localhost"). When SMTP host is absent,
//! send() is a no-op and returns Ok(false) — caller logs and continues.

use lettre::{
    message::{header::ContentType, Mailbox, SinglePart},
    transport::smtp::AsyncSmtpTransport,
    AsyncTransport, Message,
};
use lettre::Tokio1Executor;

#[derive(Clone, Debug)]
pub struct InviteMailer {
    smtp_host: String,
    smtp_port: u16,
    mail_from: String,
}

impl InviteMailer {
    pub fn from_env() -> Option<Self> {
        let host = std::env::var("MEET__SMTP_HOST").ok().filter(|v| !v.trim().is_empty())?;
        let port = std::env::var("MEET__SMTP_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(587);
        let from = std::env::var("MEET__MAIL_FROM")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "noreply@localhost".to_string());
        Some(Self { smtp_host: host, smtp_port: port, mail_from: from })
    }

    /// Send a meeting invite email. Returns Ok(true) on success, Ok(false) on
    /// smtp/build error (non-fatal — caller logs). Never propagates SMTP errors.
    pub async fn send(
        &self,
        to_email: &str,
        meeting_title: &str,
        join_url: &str,
    ) -> bool {
        match self.build_and_send(to_email, meeting_title, join_url).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, to = to_email, "invite email failed (non-fatal)");
                false
            }
        }
    }

    async fn build_and_send(
        &self,
        to_email: &str,
        meeting_title: &str,
        join_url: &str,
    ) -> anyhow::Result<()> {
        let from: lettre::Address = self.mail_from.parse()
            .map_err(|_| anyhow::anyhow!("invalid MEET__MAIL_FROM: {}", self.mail_from))?;
        let to: lettre::Address = to_email.parse()
            .map_err(|_| anyhow::anyhow!("invalid to email: {}", to_email))?;

        let body = format!(
            "You have been invited to the meeting: {meeting_title}\n\nJoin here: {join_url}"
        );

        let email = Message::builder()
            .from(Mailbox::new(None, from))
            .to(Mailbox::new(None, to))
            .subject(format!("Meeting invite: {meeting_title}"))
            .singlepart(SinglePart::builder().header(ContentType::TEXT_PLAIN).body(body))
            .map_err(|e| anyhow::anyhow!("email build failed: {e}"))?;

        let mailer: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.smtp_host)
                .port(self.smtp_port)
                .build();

        mailer.send(&email).await
            .map_err(|e| anyhow::anyhow!("smtp send failed: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_returns_none_when_smtp_host_unset() {
        std::env::remove_var("MEET__SMTP_HOST");
        assert!(InviteMailer::from_env().is_none());
    }

    #[test]
    fn from_env_returns_none_for_empty_smtp_host() {
        std::env::set_var("MEET__SMTP_HOST", "");
        assert!(InviteMailer::from_env().is_none());
        std::env::remove_var("MEET__SMTP_HOST");
    }

    #[test]
    fn from_env_returns_none_for_whitespace_smtp_host() {
        std::env::set_var("MEET__SMTP_HOST", "   ");
        assert!(InviteMailer::from_env().is_none());
        std::env::remove_var("MEET__SMTP_HOST");
    }

    #[test]
    fn from_env_some_when_smtp_host_set() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::remove_var("MEET__SMTP_PORT");
        std::env::remove_var("MEET__MAIL_FROM");
        let m = InviteMailer::from_env();
        assert!(m.is_some());
        let m = m.unwrap();
        assert_eq!(m.smtp_host, "smtp.example.com");
        std::env::remove_var("MEET__SMTP_HOST");
    }

    #[test]
    fn from_env_default_port_is_587() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::remove_var("MEET__SMTP_PORT");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.smtp_port, 587);
        std::env::remove_var("MEET__SMTP_HOST");
    }

    #[test]
    fn from_env_parses_custom_port() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::set_var("MEET__SMTP_PORT", "2525");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.smtp_port, 2525);
        std::env::remove_var("MEET__SMTP_HOST");
        std::env::remove_var("MEET__SMTP_PORT");
    }

    #[test]
    fn from_env_default_mail_from() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::remove_var("MEET__MAIL_FROM");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.mail_from, "noreply@localhost");
        std::env::remove_var("MEET__SMTP_HOST");
    }

    #[test]
    fn from_env_custom_mail_from() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::set_var("MEET__MAIL_FROM", "meet@expresso.io");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.mail_from, "meet@expresso.io");
        std::env::remove_var("MEET__SMTP_HOST");
        std::env::remove_var("MEET__MAIL_FROM");
    }

    #[test]
    fn from_env_empty_mail_from_uses_default() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::set_var("MEET__MAIL_FROM", "");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.mail_from, "noreply@localhost");
        std::env::remove_var("MEET__SMTP_HOST");
        std::env::remove_var("MEET__MAIL_FROM");
    }

    #[test]
    fn from_env_invalid_port_uses_default() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.example.com");
        std::env::set_var("MEET__SMTP_PORT", "notanumber");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.smtp_port, 587);
        std::env::remove_var("MEET__SMTP_HOST");
        std::env::remove_var("MEET__SMTP_PORT");
    }

    #[test]
    fn smtp_host_stored_correctly() {
        std::env::set_var("MEET__SMTP_HOST", "mail.internal");
        let m = InviteMailer::from_env().unwrap();
        assert_eq!(m.smtp_host, "mail.internal");
        std::env::remove_var("MEET__SMTP_HOST");
    }

    #[test]
    fn default_port_not_zero() {
        std::env::set_var("MEET__SMTP_HOST", "smtp.test");
        std::env::remove_var("MEET__SMTP_PORT");
        let m = InviteMailer::from_env().unwrap();
        assert_ne!(m.smtp_port, 0);
        std::env::remove_var("MEET__SMTP_HOST");
    }
}
