//! gov.br OIDC adapter.
//!
//! gov.br (sso.acesso.gov.br) é um provider OIDC padrão com particularidades:
//!   - Issuer:    https://sso.acesso.gov.br
//!   - Auth URL:  https://sso.acesso.gov.br/authorize
//!   - Token URL: https://sso.acesso.gov.br/token
//!   - JWKS:      https://sso.acesso.gov.br/jwk
//!   - Scopes:    `openid profile email govbr_confiabilidades`
//!   - `sub` = CPF do cidadão (hashed, 11 chars)
//!   - `amr`  = ["govbr"], `acr` = "urn:govbr:loa:{bronze|prata|ouro}"
//!
//! Integração:
//!   - Keycloak registra gov.br como external IdP (seed-realm.sh §10).
//!   - IdP mappers copiam `govbr_cpf_hash` + `govbr_confiabilidades`
//!     para o access_token emitido pelo realm expresso.
//!   - O callback do expresso-auth detecta a federação pela presença de
//!     `govbr_cpf_hash` em `AuthContext` e emite `auth.federation.govbr`
//!     (audit log estruturado). O mapping cpf_hash → tenant vive na tabela
//!     `govbr_user_map` (migration 20260426120000).
//!
//! Trilha de implementação restante (fora deste crate):
//!   - Serviço administrativo que lê `govbr_user_map` para provisionar
//!     usuários novos (tenant_id, email institucional).
//!   - UI admin para aprovar federação de novos CPFs.

#![allow(dead_code)]

use expresso_auth_client::AuthContext;

pub const GOVBR_ISSUER:         &str = "https://sso.acesso.gov.br";
pub const GOVBR_AUTH_URL:       &str = "https://sso.acesso.gov.br/authorize";
pub const GOVBR_TOKEN_URL:      &str = "https://sso.acesso.gov.br/token";
pub const GOVBR_JWKS_URL:       &str = "https://sso.acesso.gov.br/jwk";
pub const GOVBR_STAGING_ISSUER: &str = "https://sso.staging.acesso.gov.br";

pub const GOVBR_DEFAULT_SCOPES: &str = "openid profile email govbr_confiabilidades";

/// Selo de Confiabilidade gov.br (Level of Assurance).
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

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Bronze => "bronze",
            Self::Prata  => "prata",
            Self::Ouro   => "ouro",
        }
    }
}

/// Metadata about a gov.br federated identity extracted from an `AuthContext`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovbrFederation {
    /// CPF hash propagated via KC IdP mapper (never the raw CPF).
    pub cpf_hash:        String,
    /// Assurance level parsed from `acr` (None when unmapped/custom LOA).
    pub assurance:       Option<GovbrAssurance>,
    /// Raw confiabilidades list (e.g. ["cadastro-basico","validacao-biometrica"]).
    pub confiabilidades: Vec<String>,
}

impl GovbrFederation {
    /// Extract federation metadata from an `AuthContext`.
    ///
    /// Returns `Some` only when the token carries `govbr_cpf_hash` (set by the
    /// Keycloak IdP mapper on first federated login). `amr` contribution is
    /// informational only — the cpf_hash is the source of truth.
    pub fn from_ctx(ctx: &AuthContext) -> Option<Self> {
        let cpf_hash = ctx.govbr_cpf_hash.clone()?;
        let assurance = ctx.acr.as_deref().and_then(GovbrAssurance::from_acr);
        Some(Self {
            cpf_hash,
            assurance,
            confiabilidades: ctx.govbr_confiabilidades.clone(),
        })
    }

    /// Safe prefix for logging (first 8 chars of hash). Avoids leaking the full
    /// CPF hash in structured logs.
    pub fn cpf_hash_short(&self) -> &str {
        let n = self.cpf_hash.len().min(8);
        &self.cpf_hash[..n]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn ctx_with(cpf: Option<&str>, acr: Option<&str>, conf: Vec<String>) -> AuthContext {
        AuthContext {
            user_id:      Uuid::nil(),
            tenant_id:    Uuid::nil(),
            email:        "cidadao@gov.br".into(),
            display_name: "Cidadao".into(),
            roles:        vec![],
            expires_at:   0,
            acr:          acr.map(String::from),
            amr:          vec!["govbr".into()],
            govbr_cpf_hash: cpf.map(String::from),
            govbr_confiabilidades: conf,
        }
    }

    #[test]
    fn assurance_parses_all_loas() {
        assert_eq!(GovbrAssurance::from_acr("urn:govbr:loa:bronze"), Some(GovbrAssurance::Bronze));
        assert_eq!(GovbrAssurance::from_acr("urn:govbr:loa:prata"),  Some(GovbrAssurance::Prata));
        assert_eq!(GovbrAssurance::from_acr("urn:govbr:loa:ouro"),   Some(GovbrAssurance::Ouro));
        assert_eq!(GovbrAssurance::from_acr("urn:other"),            None);
    }

    #[test]
    fn federation_none_when_no_cpf_hash() {
        let c = ctx_with(None, Some("urn:govbr:loa:ouro"), vec![]);
        assert!(GovbrFederation::from_ctx(&c).is_none());
    }

    #[test]
    fn federation_extracts_all_fields() {
        let c = ctx_with(
            Some("abcdef1234567890"),
            Some("urn:govbr:loa:prata"),
            vec!["cadastro-basico".into(), "validacao-biometrica".into()],
        );
        let f = GovbrFederation::from_ctx(&c).unwrap();
        assert_eq!(f.cpf_hash, "abcdef1234567890");
        assert_eq!(f.assurance, Some(GovbrAssurance::Prata));
        assert_eq!(f.confiabilidades.len(), 2);
        assert_eq!(f.cpf_hash_short(), "abcdef12");
    }

    #[test]
    fn federation_handles_unmapped_acr() {
        let c = ctx_with(Some("hash"), Some("urn:govbr:loa:unknown"), vec![]);
        let f = GovbrFederation::from_ctx(&c).unwrap();
        assert!(f.assurance.is_none());
        assert_eq!(f.cpf_hash_short(), "hash");
    }
}
