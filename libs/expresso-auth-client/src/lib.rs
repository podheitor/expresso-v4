//! JWT validation + OIDC client for Expresso services.
//!
//! Primary entry points:
//! - `OidcConfig::new(issuer, audience)` + `OidcValidator::new(cfg).await` →
//!   async-initialized validator with cached JWKS.
//! - `Authenticated(AuthContext)` axum extractor — consumes
//!   `Authorization: Bearer <jwt>` and injects a typed context.

mod axum_ext;
mod claims;
mod error;
mod keycloak_basic;
pub mod metrics;
mod multi_validator;
mod pat_resolver;
mod tenant_resolver;
mod validator;

pub use axum_ext::{AuthRejection, Authenticated, TenantAuthenticated, ACCESS_TOKEN_COOKIE};
pub use claims::{AudClaim, AuthContext, RawClaims, RolesBlock};
pub use error::{AuthError, Result};
pub use keycloak_basic::{KcBasicAuthenticator, KcBasicConfig, KcBasicError};
pub use multi_validator::MultiRealmValidator;
pub use pat_resolver::PatResolver;
pub use tenant_resolver::TenantResolver;
pub use validator::{OidcConfig, OidcValidator};
