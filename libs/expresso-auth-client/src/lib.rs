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

pub use axum_ext::{Authenticated, AuthRejection, ACCESS_TOKEN_COOKIE};
pub use claims::{AuthContext, RawClaims, AudClaim, RolesBlock};
pub use error::{AuthError, Result};
pub use validator::{OidcConfig, OidcValidator};
