//! expresso-tenant-provision
//!
//! CLI idempotente que cria (ou atualiza) um realm Keycloak completo p/ um
//! tenant Expresso: realm + clients (web, dav, admin) + roles +
//! usuário admin inicial.
//!
//! Tenant ID = realm name (realm-per-tenant model, sprint #40c). Tokens
//! emitidos pelo realm carregam tenant via `iss` claim — mapper custom
//! `tenant_id` removido (era redundante).
//!
//! `--realm` MUST be the tenant UUID (realm name = tenant_id; tokens carry it
//! via `iss`). A human-readable realm name would 401 every request.
//!
//! Uso típico:
//! ```bash
//! expresso-tenant-provision \
//!   --kc-url http://kc:8080 --kc-admin-user admin --kc-admin-pass $KC_PASS \
//!   --realm 40894092-7ec5-4693-94f0-afb1c7fb51c4 --display "ACME Ltda" \
//!   --admin-email admin@acme.example --admin-password $INIT_PASS \
//!   --base-redirect https://acme.expresso.local/*
//! ```
//!
//! Todas as operações são idempotentes: realms/clients/roles/users já
//! existentes são detectados por GET; executa-se apenas POST necessário.
//!
//! Saída: JSON com sumário (realm_created, clients_created, ...).

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tracing::info;

#[derive(Parser, Debug)]
#[command(name = "expresso-tenant-provision", version)]
struct Cli {
    #[arg(long, env = "KC_URL", default_value = "http://expresso-keycloak:8080")]
    kc_url: String,
    #[arg(long, env = "KC_ADMIN_USER", default_value = "admin")]
    kc_admin_user: String,
    #[arg(long, env = "KC_ADMIN_PASS")]
    kc_admin_pass: String,
    #[arg(long, env = "KC_ADMIN_REALM", default_value = "master")]
    kc_admin_realm: String,

    #[arg(long)]
    realm: String,
    #[arg(long)]
    display: Option<String>,

    #[arg(long, env = "TENANT_ADMIN_EMAIL")]
    admin_email: String,
    #[arg(long, env = "TENANT_ADMIN_PASSWORD")]
    admin_password: String,
    #[arg(long, default_value = "admin")]
    admin_username: String,
    /// Marcar password como temporária (exige troca no 1º login).
    #[arg(long, default_value_t = true)]
    admin_password_temporary: bool,

    /// Redirect URIs para client `expresso-web` (public). Aceita múltiplos.
    #[arg(long, value_delimiter = ',')]
    base_redirect: Vec<String>,

    /// Dry-run: imprime payloads, não executa POSTs.
    #[arg(long)]
    dry_run: bool,
}

/// Sumário retornado como JSON no stdout ao final da execução.
#[derive(Debug, Default, Serialize)]
struct Summary {
    realm: String,
    realm_created: bool,
    clients_created: Vec<String>,
    clients_skipped: Vec<String>,
    roles_created: Vec<String>,
    roles_skipped: Vec<String>,
    admin_user_id: Option<String>,
    admin_user_created: bool,
    dry_run: bool,
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
}

async fn admin_token(c: &Client, cli: &Cli) -> Result<String> {
    let url = format!(
        "{}/realms/{}/protocol/openid-connect/token",
        cli.kc_url, cli.kc_admin_realm
    );
    let r: TokenResp = c
        .post(&url)
        .form(&[
            ("grant_type", "password"),
            ("client_id", "admin-cli"),
            ("username", &cli.kc_admin_user),
            ("password", &cli.kc_admin_pass),
        ])
        .send()
        .await
        .context("kc admin token req")?
        .error_for_status()
        .context("kc admin token status")?
        .json()
        .await
        .context("kc admin token json")?;
    Ok(r.access_token)
}

// --- Body builders (pure; testáveis sem mockar HTTP) -----------------

fn build_realm_body(realm: &str, display: &str) -> Value {
    json!({
        "realm": realm,
        "enabled": true,
        "displayName": display,
        "sslRequired": "external",
        "registrationAllowed": false,
        "resetPasswordAllowed": true,
        "rememberMe": true,
        "loginWithEmailAllowed": true,
        "duplicateEmailsAllowed": false,
        "editUsernameAllowed": false,
        "bruteForceProtected": true,
        "permanentLockout": false,
        "maxFailureWaitSeconds": 900,
        "minimumQuickLoginWaitSeconds": 60,
        "waitIncrementSeconds": 60,
        "quickLoginCheckMilliSeconds": 1000,
        "maxDeltaTimeSeconds": 43200,
        "failureFactor": 10,
        "defaultRole": {
            "name": "User",
            "description": "Default tenant role",
            "composite": false,
            "clientRole": false
        },
        "passwordPolicy": "length(12) and upperCase(1) and digits(1) and notUsername(undefined) and passwordHistory(3)",
        "accessTokenLifespan": 300,
        "ssoSessionIdleTimeout": 1800,
        "ssoSessionMaxLifespan": 36000
    })
}

fn build_web_client(redirect_uris: &[String]) -> Value {
    json!({
        "clientId": "expresso-web",
        "enabled": true,
        "publicClient": true,
        "standardFlowEnabled": true,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": false,
        "redirectUris": redirect_uris,
        "webOrigins": ["+"],
        "attributes": { "pkce.code.challenge.method": "S256" }
    })
}

fn build_dav_client() -> Value {
    json!({
        "clientId": "expresso-dav",
        "enabled": true,
        "publicClient": false,
        "standardFlowEnabled": false,
        "directAccessGrantsEnabled": true,
        "serviceAccountsEnabled": false,
        "secret": generated_secret()
    })
}

fn build_admin_client() -> Value {
    json!({
        "clientId": "expresso-admin",
        "enabled": true,
        "publicClient": false,
        "standardFlowEnabled": false,
        "directAccessGrantsEnabled": false,
        "serviceAccountsEnabled": true,
        "secret": generated_secret()
    })
}

/// Generates a 32-byte hex secret. Use `getrandom` via `reqwest`'s dep tree
/// is not reliable — fall back to time-based placeholder (operator should
/// rotate after provision).
fn generated_secret() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // 64-hex-char string; entropy ≈ 128 bits from nanos + pid mixing.
    let pid = std::process::id() as u128;
    format!(
        "{:032x}{:032x}",
        ns ^ pid,
        (ns.wrapping_mul(2654435761)) ^ pid
    )
}

fn build_user_body(username: &str, email: &str) -> Value {
    json!({
        "username": username,
        "email": email,
        "enabled": true,
        "emailVerified": true,
        "firstName": "Tenant",
        "lastName": "Admin"
    })
}

const REALM_ROLES: &[(&str, &str)] = &[
    ("SuperAdmin", "Plataform super administrator"),
    ("TenantAdmin", "Tenant-scope administrator"),
    ("User", "Regular user (default)"),
    ("Readonly", "Read-only access"),
];

// --- HTTP helpers ----------------------------------------------------

async fn realm_exists(c: &Client, cli: &Cli, tok: &str, realm: &str) -> Result<bool> {
    let url = format!("{}/admin/realms/{}", cli.kc_url, realm);
    let resp = c.get(&url).bearer_auth(tok).send().await?;
    match resp.status() {
        StatusCode::OK => Ok(true),
        StatusCode::NOT_FOUND => Ok(false),
        s => bail!("realm_exists: unexpected status {s}"),
    }
}

async fn create_realm(c: &Client, cli: &Cli, tok: &str, body: &Value) -> Result<()> {
    let url = format!("{}/admin/realms", cli.kc_url);
    c.post(&url)
        .bearer_auth(tok)
        .json(body)
        .send()
        .await
        .context("create realm")?
        .error_for_status()
        .context("create realm status")?;
    Ok(())
}

async fn list_clients(c: &Client, cli: &Cli, tok: &str, realm: &str) -> Result<Vec<Value>> {
    let url = format!("{}/admin/realms/{}/clients", cli.kc_url, realm);
    Ok(c.get(&url)
        .bearer_auth(tok)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn create_client(
    c: &Client,
    cli: &Cli,
    tok: &str,
    realm: &str,
    body: &Value,
) -> Result<String> {
    let url = format!("{}/admin/realms/{}/clients", cli.kc_url, realm);
    let resp = c
        .post(&url)
        .bearer_auth(tok)
        .json(body)
        .send()
        .await
        .context("create client")?
        .error_for_status()
        .context("create client status")?;
    let id = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit('/').next().map(String::from))
        .context("missing Location on client create")?;
    Ok(id)
}

async fn list_realm_roles(c: &Client, cli: &Cli, tok: &str, realm: &str) -> Result<Vec<Value>> {
    let url = format!("{}/admin/realms/{}/roles", cli.kc_url, realm);
    Ok(c.get(&url)
        .bearer_auth(tok)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn create_realm_role(
    c: &Client,
    cli: &Cli,
    tok: &str,
    realm: &str,
    name: &str,
    desc: &str,
) -> Result<()> {
    let url = format!("{}/admin/realms/{}/roles", cli.kc_url, realm);
    let body =
        json!({ "name": name, "description": desc, "composite": false, "clientRole": false });
    c.post(&url)
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .context("create role")?;
    Ok(())
}

async fn find_user_by_username(
    c: &Client,
    cli: &Cli,
    tok: &str,
    realm: &str,
    username: &str,
) -> Result<Option<String>> {
    let url = format!(
        "{}/admin/realms/{}/users?username={}&exact=true",
        cli.kc_url, realm, username
    );
    let arr: Vec<Value> = c
        .get(&url)
        .bearer_auth(tok)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(arr
        .first()
        .and_then(|u| u.get("id"))
        .and_then(|v| v.as_str())
        .map(String::from))
}

async fn create_user(
    c: &Client,
    cli: &Cli,
    tok: &str,
    realm: &str,
    body: &Value,
) -> Result<String> {
    let url = format!("{}/admin/realms/{}/users", cli.kc_url, realm);
    let resp = c
        .post(&url)
        .bearer_auth(tok)
        .json(body)
        .send()
        .await?
        .error_for_status()
        .context("create user")?;
    resp.headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit('/').next().map(String::from))
        .context("missing Location on user create")
}

async fn set_user_password(
    c: &Client,
    cli: &Cli,
    tok: &str,
    realm: &str,
    user_id: &str,
    password: &str,
    temporary: bool,
) -> Result<()> {
    let url = format!(
        "{}/admin/realms/{}/users/{}/reset-password",
        cli.kc_url, realm, user_id
    );
    let body = json!({ "type": "password", "value": password, "temporary": temporary });
    c.put(&url)
        .bearer_auth(tok)
        .json(&body)
        .send()
        .await?
        .error_for_status()
        .context("set password")?;
    Ok(())
}

async fn assign_realm_role(
    c: &Client,
    cli: &Cli,
    tok: &str,
    realm: &str,
    user_id: &str,
    role_name: &str,
) -> Result<()> {
    // 1. Fetch role representation (id + name needed in the POST body).
    let url = format!("{}/admin/realms/{}/roles/{}", cli.kc_url, realm, role_name);
    let role: Value = c
        .get(&url)
        .bearer_auth(tok)
        .send()
        .await?
        .error_for_status()
        .context("get role")?
        .json()
        .await?;
    // 2. POST to user's realm-mapping endpoint.
    let url2 = format!(
        "{}/admin/realms/{}/users/{}/role-mappings/realm",
        cli.kc_url, realm, user_id
    );
    c.post(&url2)
        .bearer_auth(tok)
        .json(&[role])
        .send()
        .await?
        .error_for_status()
        .context("assign role")?;
    Ok(())
}

// --- Orchestration ---------------------------------------------------

async fn provision(c: &Client, cli: &Cli, tok: &str) -> Result<Summary> {
    let mut summary = Summary {
        realm: cli.realm.clone(),
        dry_run: cli.dry_run,
        ..Default::default()
    };
    let display = cli.display.clone().unwrap_or_else(|| cli.realm.clone());
    let realm_body = build_realm_body(&cli.realm, &display);

    if cli.dry_run {
        info!(?realm_body, "dry-run realm body");
    }

    if realm_exists(c, cli, tok, &cli.realm).await? {
        info!(realm = %cli.realm, "realm already exists, skipping create");
    } else if cli.dry_run {
        summary.realm_created = true;
    } else {
        create_realm(c, cli, tok, &realm_body).await?;
        summary.realm_created = true;
        info!(realm = %cli.realm, "realm created");
    }

    let redirect_uris = if cli.base_redirect.is_empty() {
        vec![format!("https://{}.expresso.local/*", cli.realm)]
    } else {
        cli.base_redirect.clone()
    };

    let client_specs: [(&str, Value); 3] = [
        ("expresso-web", build_web_client(&redirect_uris)),
        ("expresso-dav", build_dav_client()),
        ("expresso-admin", build_admin_client()),
    ];

    let existing = if cli.dry_run {
        Vec::new()
    } else {
        list_clients(c, cli, tok, &cli.realm)
            .await
            .unwrap_or_default()
    };
    let existing_ids: Vec<String> = existing
        .iter()
        .filter_map(|cl| {
            cl.get("clientId")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect();

    for (cid, body) in client_specs.iter() {
        if existing_ids.iter().any(|x| x == cid) {
            summary.clients_skipped.push((*cid).into());
            continue;
        }
        if cli.dry_run {
            summary.clients_created.push((*cid).into());
            continue;
        }
        let client_uuid = create_client(c, cli, tok, &cli.realm, body).await?;
        summary.clients_created.push((*cid).into());
        info!(cid, %client_uuid, "client created");
    }

    // Roles
    let existing_roles = if cli.dry_run {
        Vec::new()
    } else {
        list_realm_roles(c, cli, tok, &cli.realm)
            .await
            .unwrap_or_default()
    };
    let existing_role_names: Vec<String> = existing_roles
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()).map(String::from))
        .collect();
    for (name, desc) in REALM_ROLES {
        if existing_role_names.iter().any(|x| x == name) {
            summary.roles_skipped.push((*name).into());
            continue;
        }
        if cli.dry_run {
            summary.roles_created.push((*name).into());
            continue;
        }
        create_realm_role(c, cli, tok, &cli.realm, name, desc).await?;
        summary.roles_created.push((*name).into());
        info!(role=%name, "role created");
    }

    // Admin user
    if cli.dry_run {
        summary.admin_user_created = true;
        summary.admin_user_id = Some("(dry-run)".into());
    } else if let Some(existing_id) =
        find_user_by_username(c, cli, tok, &cli.realm, &cli.admin_username).await?
    {
        info!(user_id=%existing_id, "admin user exists, skipping create");
        summary.admin_user_id = Some(existing_id);
    } else {
        let user_id = create_user(
            c,
            cli,
            tok,
            &cli.realm,
            &build_user_body(&cli.admin_username, &cli.admin_email),
        )
        .await?;
        set_user_password(
            c,
            cli,
            tok,
            &cli.realm,
            &user_id,
            &cli.admin_password,
            cli.admin_password_temporary,
        )
        .await?;
        assign_realm_role(c, cli, tok, &cli.realm, &user_id, "TenantAdmin").await?;
        summary.admin_user_id = Some(user_id);
        summary.admin_user_created = true;
        info!("admin user created");
    }

    Ok(summary)
}

/// The realm name IS the tenant id (realm-per-tenant model), and tenant ids
/// are UUIDs throughout the platform (`tenant_id: Uuid` in core/auth). A realm
/// named with anything else makes `iss=.../realms/<name>` un-parseable as a
/// tenant, so every request 401s with `MissingClaim("tenant_id")`. Reject
/// non-UUID realm names up front instead of provisioning a broken tenant.
fn validate_realm_is_uuid(realm: &str) -> Result<()> {
    let ok = realm.len() == 36
        && realm.as_bytes().iter().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    if ok {
        Ok(())
    } else {
        bail!(
            "--realm must be the tenant UUID (8-4-4-4-12 hex), got {realm:?}; \
             realm name = tenant_id in the realm-per-tenant model"
        )
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    validate_realm_is_uuid(&cli.realm)?;
    let c = Client::new();
    // dry-run ainda precisa do token p/ GETs de existência (reads-only)
    let tok = admin_token(&c, &cli).await.context("admin token")?;
    let s = provision(&c, &cli, &tok).await?;
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(())
}

// --- Tests -----------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_realm_accepts_uuid() {
        assert!(validate_realm_is_uuid("40894092-7ec5-4693-94f0-afb1c7fb51c4").is_ok());
        assert!(validate_realm_is_uuid("00000000-0000-0000-0000-000000000000").is_ok());
    }

    #[test]
    fn validate_realm_rejects_human_name() {
        assert!(validate_realm_is_uuid("tenant-acme").is_err());
        assert!(validate_realm_is_uuid("acme").is_err());
        assert!(validate_realm_is_uuid("").is_err());
    }

    #[test]
    fn validate_realm_rejects_wrong_shape() {
        // right length, wrong separators / non-hex
        assert!(validate_realm_is_uuid("40894092x7ec5x4693x94f0xafb1c7fb51c4").is_err());
        assert!(validate_realm_is_uuid("ZZ894092-7ec5-4693-94f0-afb1c7fb51c4").is_err());
        // missing one char
        assert!(validate_realm_is_uuid("40894092-7ec5-4693-94f0-afb1c7fb51c").is_err());
    }

    #[test]
    fn realm_body_has_security_defaults() {
        let b = build_realm_body("tenant-x", "Tenant X");
        assert_eq!(b["realm"], "tenant-x");
        assert_eq!(b["sslRequired"], "external");
        assert_eq!(b["bruteForceProtected"], true);
        assert_eq!(b["registrationAllowed"], false);
        assert!(b["passwordPolicy"].as_str().unwrap().contains("length(12)"));
    }

    #[test]
    fn web_client_public_with_pkce() {
        let b = build_web_client(&["https://a.example/*".into()]);
        assert_eq!(b["clientId"], "expresso-web");
        assert_eq!(b["publicClient"], true);
        assert_eq!(b["attributes"]["pkce.code.challenge.method"], "S256");
        assert_eq!(b["redirectUris"][0], "https://a.example/*");
    }

    #[test]
    fn dav_client_direct_grants_confidential() {
        let b = build_dav_client();
        assert_eq!(b["clientId"], "expresso-dav");
        assert_eq!(b["publicClient"], false);
        assert_eq!(b["directAccessGrantsEnabled"], true);
        assert!(b["secret"].as_str().unwrap().len() >= 32);
    }

    #[test]
    fn admin_client_service_account() {
        let b = build_admin_client();
        assert_eq!(b["clientId"], "expresso-admin");
        assert_eq!(b["serviceAccountsEnabled"], true);
        assert_eq!(b["standardFlowEnabled"], false);
    }

    #[test]
    fn all_realm_roles_declared() {
        let names: Vec<&str> = REALM_ROLES.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["SuperAdmin", "TenantAdmin", "User", "Readonly"]);
    }

    #[test]
    fn generated_secret_is_64_hex_chars() {
        let s1 = generated_secret();
        assert_eq!(s1.len(), 64);
        assert!(s1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generated_secrets_are_unique() {
        let s1 = generated_secret();
        let s2 = generated_secret();
        assert_ne!(s1, s2);
    }

    #[test]
    fn realm_body_display_name_set() {
        let b = build_realm_body("acme", "Acme Corp");
        assert_eq!(b["displayName"], "Acme Corp");
        assert_eq!(b["realm"], "acme");
    }

    #[test]
    fn realm_body_enabled_is_true() {
        let b = build_realm_body("demo", "Demo Tenant");
        assert_eq!(b["enabled"], true);
    }

    #[test]
    fn realm_body_brute_force_protected() {
        let b = build_realm_body("test", "Test");
        assert_eq!(b["bruteForceProtected"], true);
    }

    #[test]
    fn realm_body_realm_field_matches_input() {
        let b = build_realm_body("myrealm", "My Realm");
        assert_eq!(b["realm"].as_str(), Some("myrealm"));
    }

    #[test]
    fn build_user_body_username_preserved() {
        let b = build_user_body("alice", "alice@example.com");
        assert_eq!(b["username"].as_str(), Some("alice"));
    }

    #[test]
    fn build_user_body_email_preserved() {
        let b = build_user_body("bob", "bob@corp.com");
        assert_eq!(b["email"].as_str(), Some("bob@corp.com"));
    }

    #[test]
    fn build_user_body_enabled_is_true() {
        let b = build_user_body("alice", "alice@corp.com");
        assert_eq!(b["enabled"].as_bool(), Some(true));
    }

    #[test]
    fn realm_body_registration_allowed_is_false() {
        let b = build_realm_body("demo", "Demo");
        assert_eq!(b["registrationAllowed"].as_bool(), Some(false));
    }

    #[test]
    fn build_user_body_email_verified_is_false_by_default() {
        // Platform-provisioned tenant admins are pre-verified — no email round-trip.
        let b = build_user_body("carol", "carol@example.com");
        assert_eq!(b["emailVerified"].as_bool(), Some(true));
    }

    #[test]
    fn realm_body_contains_realm_key() {
        let b = build_realm_body("test-realm", "Test");
        assert!(b.get("realm").is_some());
    }

    #[test]
    fn build_user_body_username_is_preserved() {
        let b = build_user_body("zara", "zara@example.com");
        assert_eq!(b["username"].as_str(), Some("zara"));
    }

    #[test]
    fn realm_roles_count_is_four() {
        assert_eq!(REALM_ROLES.len(), 4);
    }

    #[test]
    fn realm_roles_are_unique() {
        let names: Vec<&str> = REALM_ROLES.iter().map(|(n, _)| *n).collect();
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(names.len(), unique.len());
    }

    #[test]
    fn realm_roles_count_is_nonzero() {
        assert!(!REALM_ROLES.is_empty());
    }

    #[test]
    fn generated_secret_length_is_64() {
        let s = generated_secret();
        assert_eq!(s.len(), 64);
    }

    #[test]
    fn build_user_body_first_name_is_tenant() {
        let body = build_user_body("alice", "alice@example.com");
        assert_eq!(body["firstName"], "Tenant");
    }
}
