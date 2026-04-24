//! expresso-tenant-migrate
//!
//! Lê usuários do realm origem (single-tenant, ex `expresso`) e os replica
//! para realms de destino determinados pelo atributo `tenant_id` de cada
//! usuário. Idempotente: já existentes (match por username) são pulados.
//!
//! O que é copiado:
//! - username, email, firstName, lastName, emailVerified, enabled
//! - atributos custom (exceto tenant_id, que vira intrínseco do realm)
//! - realm roles (mapeamento 1:1 por nome — realm destino DEVE ter roles
//!   criadas previamente via `expresso-tenant-provision`)
//!
//! O que NÃO é copiado:
//! - Password hash (cada realm tem sua key; user recebe reset)
//! - TOTP/WebAuthn credentials (não exportáveis pela admin API)
//! - Grupos (fora de scope; usar --require-group se necessário)
//!
//! Modo reset: `--send-reset` dispara `UPDATE_PASSWORD` + email verify via
//! execute-actions-email no realm destino.
//!
//! Saída: JSON summary com per-tenant stats.

use anyhow::{bail, Context, Result};
use clap::Parser;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use tracing::{info, warn};

#[derive(Parser, Debug)]
#[command(name = "expresso-tenant-migrate", version)]
struct Cli {
    #[arg(long, env = "KC_URL", default_value = "http://expresso-keycloak:8080")]
    kc_url: String,
    #[arg(long, env = "KC_ADMIN_USER", default_value = "admin")]
    kc_admin_user: String,
    #[arg(long, env = "KC_ADMIN_PASS")]
    kc_admin_pass: String,
    #[arg(long, env = "KC_ADMIN_REALM", default_value = "master")]
    kc_admin_realm: String,

    /// Realm origem (source).
    #[arg(long, default_value = "expresso")]
    source_realm: String,

    /// Somente migra users cujo tenant_id esteja nessa lista (CSV). Vazio = todos.
    #[arg(long, value_delimiter = ',')]
    only_tenants: Vec<String>,

    /// Envia email UPDATE_PASSWORD + VERIFY_EMAIL para cada user migrado.
    #[arg(long)]
    send_reset: bool,

    /// Executa as operações de escrita. Default = dry-run.
    #[arg(long)]
    apply: bool,

    /// Batch size na paginação de users.
    #[arg(long, default_value_t = 100)]
    page_size: u32,
}

#[derive(Debug, Default, Serialize)]
struct Summary {
    source_realm: String,
    applied: bool,
    users_scanned: usize,
    users_missing_tenant: usize,
    per_tenant: BTreeMap<String, TenantStats>,
}

#[derive(Debug, Default, Serialize, Clone)]
struct TenantStats {
    realm_exists: bool,
    users_created: usize,
    users_skipped: usize,
    roles_assigned: usize,
    password_reset_sent: usize,
    errors: usize,
}

#[derive(Deserialize)]
struct TokenResp { access_token: String }

async fn admin_token(c: &Client, cli: &Cli) -> Result<String> {
    let url = format!("{}/realms/{}/protocol/openid-connect/token", cli.kc_url, cli.kc_admin_realm);
    let r: TokenResp = c.post(&url)
        .form(&[
            ("grant_type", "password"),
            ("client_id",  "admin-cli"),
            ("username",   &cli.kc_admin_user),
            ("password",   &cli.kc_admin_pass),
        ])
        .send().await.context("kc admin token")?
        .error_for_status().context("kc admin token status")?
        .json().await?;
    Ok(r.access_token)
}

/// Fetch users from `realm` paginating until empty.
async fn fetch_all_users(c: &Client, tok: &str, kc_url: &str, realm: &str, page_size: u32) -> Result<Vec<Value>> {
    let mut out = Vec::new();
    let mut first = 0u32;
    loop {
        let url = format!("{}/admin/realms/{}/users?first={}&max={}", kc_url, realm, first, page_size);
        let page: Vec<Value> = c.get(&url).bearer_auth(tok).send().await?
            .error_for_status()?.json().await?;
        let n = page.len() as u32;
        out.extend(page);
        if n < page_size { break; }
        first += n;
    }
    Ok(out)
}

async fn realm_exists(c: &Client, tok: &str, kc_url: &str, realm: &str) -> Result<bool> {
    let url = format!("{}/admin/realms/{}", kc_url, realm);
    let resp = c.get(&url).bearer_auth(tok).send().await?;
    match resp.status() {
        StatusCode::OK => Ok(true),
        StatusCode::NOT_FOUND => Ok(false),
        s => bail!("realm_exists: unexpected {s}"),
    }
}

async fn user_exists(c: &Client, tok: &str, kc_url: &str, realm: &str, username: &str) -> Result<Option<String>> {
    let url = format!("{}/admin/realms/{}/users?username={}&exact=true", kc_url, realm, username);
    let list: Vec<Value> = c.get(&url).bearer_auth(tok).send().await?
        .error_for_status()?.json().await?;
    Ok(list.first().and_then(|u| u["id"].as_str().map(String::from)))
}

async fn create_user(c: &Client, tok: &str, kc_url: &str, realm: &str, body: &Value) -> Result<String> {
    let url = format!("{}/admin/realms/{}/users", kc_url, realm);
    let resp = c.post(&url).bearer_auth(tok).json(body).send().await?;
    if !resp.status().is_success() {
        let s = resp.status();
        let t = resp.text().await.unwrap_or_default();
        bail!("create_user {s}: {t}");
    }
    let loc = resp.headers().get("Location").and_then(|h| h.to_str().ok()).unwrap_or("");
    Ok(loc.rsplit('/').next().unwrap_or("").to_string())
}

async fn user_realm_roles(c: &Client, tok: &str, kc_url: &str, realm: &str, user_id: &str) -> Result<Vec<Value>> {
    let url = format!("{}/admin/realms/{}/users/{}/role-mappings/realm", kc_url, realm, user_id);
    let v: Vec<Value> = c.get(&url).bearer_auth(tok).send().await?
        .error_for_status()?.json().await?;
    Ok(v)
}

async fn list_realm_roles(c: &Client, tok: &str, kc_url: &str, realm: &str) -> Result<Vec<Value>> {
    let url = format!("{}/admin/realms/{}/roles", kc_url, realm);
    let v: Vec<Value> = c.get(&url).bearer_auth(tok).send().await?
        .error_for_status()?.json().await?;
    Ok(v)
}

async fn assign_roles(c: &Client, tok: &str, kc_url: &str, realm: &str, user_id: &str, roles: &[Value]) -> Result<()> {
    if roles.is_empty() { return Ok(()); }
    let url = format!("{}/admin/realms/{}/users/{}/role-mappings/realm", kc_url, realm, user_id);
    c.post(&url).bearer_auth(tok).json(roles).send().await?.error_for_status()?;
    Ok(())
}

async fn send_execute_actions(c: &Client, tok: &str, kc_url: &str, realm: &str, user_id: &str) -> Result<()> {
    let url = format!("{}/admin/realms/{}/users/{}/execute-actions-email", kc_url, realm, user_id);
    let actions = json!(["UPDATE_PASSWORD", "VERIFY_EMAIL"]);
    c.put(&url).bearer_auth(tok).json(&actions).send().await?.error_for_status()?;
    Ok(())
}

/// Extrai tenant_id do attribute bag; toma o primeiro valor não vazio.
fn extract_tenant_id(user: &Value) -> Option<String> {
    user.get("attributes")?
        .get("tenant_id")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(String::from)
}

/// Constrói body de criação no realm destino a partir do user source.
fn build_user_payload(source: &Value) -> Value {
    // Copia attributes sem tenant_id (agora é intrínseco do realm).
    let attrs = source.get("attributes").and_then(|v| v.as_object()).map(|m| {
        let mut clean = serde_json::Map::new();
        for (k, v) in m.iter() {
            if k == "tenant_id" { continue; }
            clean.insert(k.clone(), v.clone());
        }
        Value::Object(clean)
    }).unwrap_or(Value::Object(Default::default()));
    json!({
        "username":       source["username"],
        "email":          source["email"],
        "firstName":      source["firstName"],
        "lastName":       source["lastName"],
        "enabled":        source["enabled"].as_bool().unwrap_or(true),
        "emailVerified":  source["emailVerified"].as_bool().unwrap_or(false),
        "attributes":     attrs,
    })
}

/// Filtra roles a serem replicadas — apenas as que existem no realm destino
/// por nome. `dest_roles` = listagem completa do realm destino (com id).
fn roles_to_assign(source_roles: &[Value], dest_roles: &[Value]) -> Vec<Value> {
    let dest_by_name: std::collections::HashMap<&str, &Value> = dest_roles.iter()
        .filter_map(|r| r["name"].as_str().map(|n| (n, r)))
        .collect();
    source_roles.iter()
        .filter_map(|r| r["name"].as_str())
        .filter_map(|name| dest_by_name.get(name).map(|r| (*r).clone()))
        .collect()
}

async fn run(c: Client, cli: Cli, tok: String) -> Result<Summary> {
    info!(source = %cli.source_realm, "scanning source realm");
    let users = fetch_all_users(&c, &tok, &cli.kc_url, &cli.source_realm, cli.page_size).await?;
    info!(count = users.len(), "source users loaded");

    let only: HashSet<String> = cli.only_tenants.iter().cloned().collect();
    let mut sum = Summary {
        source_realm: cli.source_realm.clone(),
        applied: cli.apply,
        users_scanned: users.len(),
        ..Default::default()
    };

    // Group by tenant_id
    let mut by_tenant: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for u in users {
        match extract_tenant_id(&u) {
            Some(t) if only.is_empty() || only.contains(&t) => {
                by_tenant.entry(t).or_default().push(u);
            }
            Some(_) => {} // skipped by filter
            None => { sum.users_missing_tenant += 1; warn!(username = ?u["username"], "user missing tenant_id attribute"); }
        }
    }

    for (tenant, tenant_users) in by_tenant.iter() {
        let mut ts = TenantStats::default();
        ts.realm_exists = realm_exists(&c, &tok, &cli.kc_url, tenant).await.unwrap_or(false);
        if !ts.realm_exists {
            warn!(%tenant, "destination realm missing — run expresso-tenant-provision first");
            sum.per_tenant.insert(tenant.clone(), ts);
            continue;
        }
        let dest_roles = list_realm_roles(&c, &tok, &cli.kc_url, tenant).await.unwrap_or_default();

        for u in tenant_users {
            let username = u["username"].as_str().unwrap_or("");
            if username.is_empty() { ts.errors += 1; continue; }

            match user_exists(&c, &tok, &cli.kc_url, tenant, username).await {
                Ok(Some(_)) => { ts.users_skipped += 1; continue; }
                Ok(None)    => {}
                Err(e)      => { warn!(%username, %tenant, error = %e, "user_exists failed"); ts.errors += 1; continue; }
            }

            let payload = build_user_payload(u);
            if !cli.apply {
                info!(%username, %tenant, "DRY: would create + assign roles");
                ts.users_created += 1;
                continue;
            }

            let new_id = match create_user(&c, &tok, &cli.kc_url, tenant, &payload).await {
                Ok(id) => { ts.users_created += 1; id }
                Err(e) => { warn!(%username, %tenant, error = %e, "create_user failed"); ts.errors += 1; continue; }
            };

            if let Some(source_id) = u["id"].as_str() {
                if let Ok(src_roles) = user_realm_roles(&c, &tok, &cli.kc_url, &cli.source_realm, source_id).await {
                    let mapped = roles_to_assign(&src_roles, &dest_roles);
                    let n = mapped.len();
                    if let Err(e) = assign_roles(&c, &tok, &cli.kc_url, tenant, &new_id, &mapped).await {
                        warn!(%username, %tenant, error = %e, "assign_roles failed");
                        ts.errors += 1;
                    } else {
                        ts.roles_assigned += n;
                    }
                }
            }

            if cli.send_reset {
                if let Err(e) = send_execute_actions(&c, &tok, &cli.kc_url, tenant, &new_id).await {
                    warn!(%username, %tenant, error = %e, "execute-actions-email failed");
                    ts.errors += 1;
                } else {
                    ts.password_reset_sent += 1;
                }
            }
        }
        sum.per_tenant.insert(tenant.clone(), ts);
    }

    Ok(sum)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();
    let c = Client::new();
    let tok = admin_token(&c, &cli).await.context("admin token")?;
    let s = run(c, cli, tok).await?;
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tenant_id_from_attributes() {
        let u = json!({"attributes": {"tenant_id": ["acme"]}});
        assert_eq!(extract_tenant_id(&u).as_deref(), Some("acme"));
    }

    #[test]
    fn missing_tenant_id_returns_none() {
        let u = json!({"attributes": {"other": ["x"]}});
        assert!(extract_tenant_id(&u).is_none());

        let u2 = json!({"attributes": {"tenant_id": []}});
        assert!(extract_tenant_id(&u2).is_none());

        let u3 = json!({"attributes": {"tenant_id": [""]}});
        assert!(extract_tenant_id(&u3).is_none());

        let u4 = json!({});
        assert!(extract_tenant_id(&u4).is_none());
    }

    #[test]
    fn builds_payload_without_tenant_attr() {
        let src = json!({
            "username": "alice",
            "email":    "alice@ex",
            "firstName":"Alice",
            "lastName": "Lee",
            "enabled":  true,
            "emailVerified": true,
            "attributes": {
                "tenant_id": ["acme"],
                "locale": ["pt-BR"]
            }
        });
        let out = build_user_payload(&src);
        assert_eq!(out["username"], "alice");
        assert_eq!(out["emailVerified"], true);
        assert!(out["attributes"]["tenant_id"].is_null());
        assert_eq!(out["attributes"]["locale"][0], "pt-BR");
    }

    #[test]
    fn roles_to_assign_intersects_by_name() {
        let src = vec![
            json!({"name": "TenantAdmin"}),
            json!({"name": "User"}),
            json!({"name": "Legacy"}),
        ];
        let dest = vec![
            json!({"id": "1", "name": "TenantAdmin"}),
            json!({"id": "2", "name": "User"}),
        ];
        let out = roles_to_assign(&src, &dest);
        assert_eq!(out.len(), 2);
        let names: Vec<&str> = out.iter().map(|r| r["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"TenantAdmin"));
        assert!(names.contains(&"User"));
    }

    #[test]
    fn empty_dest_roles_assigns_nothing() {
        let src = vec![json!({"name": "X"})];
        assert!(roles_to_assign(&src, &[]).is_empty());
    }
}
