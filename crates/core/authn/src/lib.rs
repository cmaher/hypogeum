//! Authentication contracts shared by Hypogeum applications: the request
//! [`Principal`] and the extractors that read it. An application middleware
//! verifies the session and inserts the `Principal` into request extensions;
//! handlers only ever read it from there.
pub mod error;
pub mod extractor;
pub mod principal;

pub use error::AuthError;
pub use extractor::{AuthRejection, OptionalAuth, RequireAuth};
pub use hypgm_errors::{PublicError, PublicErrorInfo};
pub use principal::Principal;
