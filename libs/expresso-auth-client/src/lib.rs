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
mod validator;
mod keycloak_basic;
mod tenant_resolver;
mod multi_validator;
pub mod metrics;

pub use axum_ext::{Authenticated, AuthRejection, TenantAuthenticated, ACCESS_TOKEN_COOKIE};
pub use claims::{AuthContext, RawClaims, AudClaim, RolesBlock};
pub use error::{AuthError, Result};
pub use validator::{OidcConfig, OidcValidator};
pub use keycloak_basic::{KcBasicAuthenticator, KcBasicConfig, KcBasicError};
pub use tenant_resolver::TenantResolver;
pub use multi_validator::MultiRealmValidator;
