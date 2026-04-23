//! App config loaded from env vars + optional config file
//! Precedence (high→low): env vars → config.toml → defaults

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub s3: S3Config,
    pub nats: NatsConfig,
    pub keycloak: KeycloakConfig,
    pub telemetry: TelemetryConfig,
    pub mail_server: MailServerConfig,
    /// URL of the search service (e.g. "http://localhost:8007")
    #[serde(default)]
    pub search_url: String,
    /// URL of the calendar service (e.g. "http://localhost:8002").
    /// When set, `expresso-mail` forwards iMIP REPLY parts to
    /// `{calendar_url}/api/v1/scheduling/inbox` on delivery.
    #[serde(default)]
    pub calendar_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// HTTP API listen address
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Graceful shutdown timeout in seconds
    #[serde(default = "default_shutdown_timeout")]
    pub shutdown_timeout_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    #[serde(default = "default_db_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_db_min_connections")]
    pub min_connections: u32,
    /// Connection acquire timeout in seconds
    #[serde(default = "default_db_acquire_timeout")]
    pub acquire_timeout_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub url: String,
    #[serde(default = "default_redis_pool_size")]
    pub pool_size: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct S3Config {
    pub endpoint: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    #[serde(default = "default_s3_region")]
    pub region: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NatsConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct KeycloakConfig {
    pub url: String,
    pub realm: String,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TelemetryConfig {
    #[serde(default = "default_otlp_endpoint")]
    pub otlp_endpoint: String,
    #[serde(default)]
    pub log_json: bool,
    /// log level filter — e.g. "info,expresso=debug"
    #[serde(default = "default_log_filter")]
    pub log_filter: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MailServerConfig {
    /// SMTP listen port (usually 25 internal, 587 submission)
    #[serde(default = "default_smtp_port")]
    pub smtp_port: u16,
    /// LMTP listen port (Postfix → app delivery, usually 24)
    #[serde(default = "default_lmtp_port")]
    pub lmtp_port: u16,
    /// IMAP listen port
    #[serde(default = "default_imap_port")]
    pub imap_port: u16,
    /// IMAPS port
    #[serde(default = "default_imaps_port")]
    pub imaps_port: u16,
    /// Outbound relay host (submission)
    #[serde(default = "default_relay_host")]
    pub relay_host: String,
    /// Outbound relay port (587 submission, 2525 alt)
    #[serde(default = "default_relay_port")]
    pub relay_port: u16,
    pub domain: String,
    /// TLS cert path (PEM)
    pub tls_cert: Option<String>,
    /// TLS key path (PEM)
    pub tls_key: Option<String>,
    /// DKIM selector name (e.g. "default")
    #[serde(default)]
    pub dkim_selector: Option<String>,
    /// Path to DKIM RSA private key (PEM)
    #[serde(default)]
    pub dkim_key_path: Option<String>,
}

// ─── Defaults ────────────────────────────────────────────────────────────────

fn default_host() -> String          { "0.0.0.0".into() }
fn default_port() -> u16             { 8000 }
fn default_shutdown_timeout() -> u64 { 30 }
fn default_db_max_connections() -> u32  { 20 }
fn default_db_min_connections() -> u32  { 2 }
fn default_db_acquire_timeout() -> u64  { 5 }
fn default_redis_pool_size() -> usize   { 10 }
fn default_s3_region() -> String        { "us-east-1".into() }
fn default_otlp_endpoint() -> String    { "http://localhost:4317".into() }
fn default_log_filter() -> String       { "info".into() }
fn default_smtp_port() -> u16           { 25 }
fn default_lmtp_port() -> u16           { 24 }
fn default_imap_port() -> u16           { 143 }
fn default_imaps_port() -> u16          { 993 }
fn default_relay_host() -> String     { "127.0.0.1".into() }
fn default_relay_port() -> u16          { 587 }

// ─── Loader ──────────────────────────────────────────────────────────────────

impl AppConfig {
    /// Load config: env vars override config file, both override defaults.
    /// Env var prefix: `EXPRESSO__` + section + `__` + key in SCREAMING_SNAKE.
    /// e.g. DATABASE__URL, SERVER__PORT, S3__BUCKET
    pub fn from_env() -> crate::error::Result<Self> {
        let cfg = config::Config::builder()
            .add_source(
                config::Environment::default()
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .map_err(crate::error::CoreError::Config)?
            .try_deserialize()
            .map_err(crate::error::CoreError::Config)?;

        Ok(cfg)
    }
}
