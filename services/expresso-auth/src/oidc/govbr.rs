//! gov.br OIDC adapter — STUB.
//!
//! gov.br (sso.acesso.gov.br) é um provider OIDC padrão com particularidades:
//!   - Issuer:    https://sso.acesso.gov.br
//!   - Auth URL:  https://sso.acesso.gov.br/authorize
//!   - Token URL: https://sso.acesso.gov.br/token
//!   - JWKS:      https://sso.acesso.gov.br/jwk
//!   - Scopes:    `openid profile email govbr_confiabilidades`
//!   - `sub` = CPF do cidadão (hashed, 11 chars)
//!   - `amr`  = ["govbr"], `acr` = nível ouro/prata/bronze
//!
//! Mapping:
//!   - Registrar como external IdP no Keycloak (kc add-broker gov.br) →
//!     realm "expresso" continua como RP único para o webmail.
//!   - Custom claim mapper: sub gov.br → attribute `govbr_cpf_hash`.
//!   - Tenant resolution: pode ficar no callback via lookup em tabela
//!     `cpf_hash → tenant_id` (migration pendente).
//!
//! Implementation plan (≠ ainda implementado):
//!   1. Seed do realm-level identity-provider via kcadm (`Federated Gov.br`).
//!   2. Protocol mappers para preservar claims gov.br-específicas.
//!   3. Migration `govbr_user_map` para associar sub → tenant.
//!   4. Custom authenticator que preenche `tenant_id` na primeira federação.
//!   5. Audit log em `audit.federation.govbr` para cada SSO.

#![allow(dead_code)]

pub const GOVBR_ISSUER:       &str = "https://sso.acesso.gov.br";
pub const GOVBR_AUTH_URL:     &str = "https://sso.acesso.gov.br/authorize";
pub const GOVBR_TOKEN_URL:    &str = "https://sso.acesso.gov.br/token";
pub const GOVBR_JWKS_URL:     &str = "https://sso.acesso.gov.br/jwk";
pub const GOVBR_STAGING_ISSUER: &str = "https://sso.staging.acesso.gov.br";

pub const GOVBR_DEFAULT_SCOPES: &str = "openid profile email govbr_confiabilidades";

/// Assurance level (gov.br "Selo de Confiabilidade").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GovbrAssurance { Bronze, Prata, Ouro }

impl GovbrAssurance {
    pub fn from_acr(acr: &str) -> Option<Self> {
        match acr {
            "urn:govbr:loa:bronze" => Some(Self::Bronze),
            "urn:govbr:loa:prata"  => Some(Self::Prata),
            "urn:govbr:loa:ouro"   => Some(Self::Ouro),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assurance_parses_all_loas() {
        assert_eq!(GovbrAssurance::from_acr("urn:govbr:loa:bronze"), Some(GovbrAssurance::Bronze));
        assert_eq!(GovbrAssurance::from_acr("urn:govbr:loa:prata"),  Some(GovbrAssurance::Prata));
        assert_eq!(GovbrAssurance::from_acr("urn:govbr:loa:ouro"),   Some(GovbrAssurance::Ouro));
        assert_eq!(GovbrAssurance::from_acr("urn:other"),            None);
    }
}
