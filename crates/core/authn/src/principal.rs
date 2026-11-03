use uuid::Uuid;

/// The authenticated caller of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Principal {
    /// The verified session that authenticated the request.
    pub session_id: Uuid,
    /// The application user the session belongs to.
    pub user_id: Uuid,
}

impl Principal {
    pub const fn new(session_id: Uuid, user_id: Uuid) -> Self {
        Self {
            session_id,
            user_id,
        }
    }
}
