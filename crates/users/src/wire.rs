use crate::models::User;
use chrono::{DateTime, Utc};
use errors::{RepoError, ServiceError};
use rpc::users as pb;
use uuid::Uuid;

pub(crate) fn id(value: &str) -> Result<Uuid, RepoError> {
    Uuid::parse_str(value).map_err(|_| RepoError::InvalidInput("Expected a UUID".into()))
}
pub(crate) fn to_wire(user: User) -> pb::User {
    let timestamp = |value: DateTime<Utc>| prost_types::Timestamp {
        seconds: value.timestamp(),
        nanos: value.timestamp_subsec_nanos() as i32,
    };
    pb::User {
        id: user.id.to_string(),
        display_name: user.display_name,
        created_at: Some(timestamp(user.created_at)),
        updated_at: Some(timestamp(user.updated_at)),
    }
}
pub(crate) fn from_wire(user: pb::User) -> Result<User, ServiceError> {
    let timestamp = |value: Option<prost_types::Timestamp>| {
        let value = value.ok_or(ServiceError::InvalidResponse("Missing timestamp"))?;
        if !(0..1_000_000_000).contains(&value.nanos)
            || !(-62135596800..=253402300799).contains(&value.seconds)
        {
            return Err(ServiceError::InvalidResponse("Invalid timestamp"));
        }
        DateTime::from_timestamp(value.seconds, value.nanos as u32)
            .ok_or(ServiceError::InvalidResponse("Invalid timestamp"))
    };
    Ok(User {
        id: id(&user.id).map_err(|_| ServiceError::InvalidResponse("Invalid UUID"))?,
        display_name: user.display_name,
        created_at: timestamp(user.created_at)?,
        updated_at: timestamp(user.updated_at)?,
    })
}
