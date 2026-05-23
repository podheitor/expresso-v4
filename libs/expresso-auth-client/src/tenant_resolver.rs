//! Tenant resolver — maps incoming Host header → realm name.
//!
//! Fase 2 do realm-per-tenant: cada tenant vive em seu próprio realm
//! Keycloak. Serviços precisam descobrir QUAL realm validar antes de
//! aceitar o JWT; a chave é o Host HTTP que atendeu a requisição.
//!
//! Configuração: env var no formato `host1:realm1,host2:realm2`. Hosts
//! são normalizados (lowercase, sem porta).

use std::collections::HashMap;

#[derive(Debug, Clone, Default)]
pub struct TenantResolver {
    map: HashMap<String, String>,
}

impl TenantResolver {
    /// Build from a plain `host:realm,host:realm` string. Entries inválidas
    /// são ignoradas silenciosamente (log pelo chamador se necessário).
    pub fn parse(raw: &str) -> Self {
        let mut map = HashMap::new();
        for entry in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some((host, realm)) = entry.split_once(':') {
                let h = normalize_host(host);
                let r = realm.trim().to_string();
                if !h.is_empty() && !r.is_empty() {
                    map.insert(h, r);
                }
            }
        }
        Self { map }
    }

    /// Build from env var. Returns empty resolver if var unset/empty.
    pub fn from_env(var: &str) -> Self {
        std::env::var(var).map(|v| Self::parse(&v)).unwrap_or_default()
    }

    /// Resolve host → realm name. Aceita "acme.example.com:443" etc.
    pub fn resolve(&self, host: &str) -> Option<&str> {
        self.map.get(&normalize_host(host)).map(String::as_str)
    }

    pub fn len(&self) -> usize { self.map.len() }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }

    /// All known hosts (for debug/metrics).
    pub fn hosts(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(String::as_str)
    }
}

/// Lowercase + strip port.
fn normalize_host(raw: &str) -> String {
    let h = raw.split(':').next().unwrap_or("").trim();
    h.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_pair() {
        let r = TenantResolver::parse("acme.example.com:acme");
        assert_eq!(r.resolve("acme.example.com"), Some("acme"));
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn parses_multiple_entries() {
        let r = TenantResolver::parse("a.ex:a, b.ex:b,c.ex:c");
        assert_eq!(r.resolve("a.ex"), Some("a"));
        assert_eq!(r.resolve("B.EX"), Some("b")); // case-insensitive
        assert_eq!(r.resolve("c.ex:8443"), Some("c")); // port stripped
    }

    #[test]
    fn unknown_host_returns_none() {
        let r = TenantResolver::parse("a:x");
        assert!(r.resolve("b").is_none());
    }

    #[test]
    fn empty_input_is_empty_resolver() {
        assert!(TenantResolver::parse("").is_empty());
        assert!(TenantResolver::parse(" , , ").is_empty());
    }

    #[test]
    fn ignores_malformed_entries() {
        let r = TenantResolver::parse("valid.ex:realm1,nocolonhere,:emptyhost,host:");
        assert_eq!(r.len(), 1);
        assert_eq!(r.resolve("valid.ex"), Some("realm1"));
    }

    #[test]
    fn resolve_strips_port_from_host() {
        let r = TenantResolver::parse("app.example.com:my-realm");
        assert_eq!(r.resolve("app.example.com:443"), Some("my-realm"));
    }

    #[test]
    fn len_reflects_count_of_valid_entries() {
        let r = TenantResolver::parse("a.x:r1,b.x:r2,c.x:r3");
        assert_eq!(r.len(), 3);
        assert!(!r.is_empty());
    }

    #[test]
    fn resolve_case_insensitive() {
        let r = TenantResolver::parse("Tenant.Local:t1");
        assert_eq!(r.resolve("tenant.local"), Some("t1"));
        assert_eq!(r.resolve("TENANT.LOCAL"), Some("t1"));
    }

    #[test]
    fn resolve_unknown_host_is_none() {
        let r = TenantResolver::parse("a.x:r1");
        assert!(r.resolve("unknown.host").is_none());
    }

    #[test]
    fn empty_raw_resolves_nothing() {
        let r = TenantResolver::parse("");
        assert!(r.resolve("any.host").is_none());
    }

    #[test]
    fn multiple_entries_resolve_correct_realm() {
        let r = TenantResolver::parse("a.x:realm_a,b.y:realm_b");
        assert_eq!(r.resolve("a.x"), Some("realm_a"));
        assert_eq!(r.resolve("b.y"), Some("realm_b"));
    }

    #[test]
    fn unknown_host_resolves_to_none() {
        let r = TenantResolver::parse("a.x:realm_a");
        assert_eq!(r.resolve("unknown.host"), None);
    }

    #[test]
    fn empty_input_resolves_nothing() {
        let r = TenantResolver::parse("");
        assert!(r.resolve("any.host").is_none());
    }

    #[test]
    fn whitespace_only_input_resolves_nothing() {
        let r = TenantResolver::parse("   ");
        assert!(r.resolve("any.host").is_none());
    }

    #[test]
    fn resolver_is_empty_for_blank_config() {
        let r = TenantResolver::parse("");
        assert!(r.is_empty());
    }

    #[test]
    fn normalize_host_strips_port_on_resolve() {
        let r = TenantResolver::parse("acme.example.com:corp");
        // Host with port should still resolve — normalize_host strips the port.
        assert_eq!(r.resolve("acme.example.com:443"), Some("corp"));
    }

    #[test]
    fn len_matches_number_of_valid_entries() {
        let r = TenantResolver::parse("a.example.com:realm1,b.example.com:realm2");
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn hosts_iterator_yields_all_normalized_hosts() {
        let r = TenantResolver::parse("Alpha.Local:realm_a,Beta.Local:realm_b");
        let mut hosts: Vec<&str> = r.hosts().collect();
        hosts.sort();
        assert_eq!(hosts, vec!["alpha.local", "beta.local"]);
    }

    #[test]
    fn single_entry_hosts_iterator_yields_one_host() {
        let r = TenantResolver::parse("mail.example.com:my_realm");
        let hosts: Vec<&str> = r.hosts().collect();
        assert_eq!(hosts.len(), 1);
    }

    #[test]
    fn default_resolver_is_empty() {
        let r = TenantResolver::default();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    #[test]
    fn resolve_returns_correct_realm_for_uppercase_host() {
        let r = TenantResolver::parse("CORP.EXAMPLE.COM:corp-realm");
        assert_eq!(r.resolve("corp.example.com"), Some("corp-realm"));
    }

    #[test]
    fn parse_single_entry_len_is_one() {
        let r = TenantResolver::parse("host.example.com:tenantrealm");
        assert_eq!(r.len(), 1);
    }
}
