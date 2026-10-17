#![cfg(feature = "tonic")]
use hypgm_errors::{RepoError, ServiceError};

#[test]
fn transport_reports_input_and_missing_records_without_exposing_internals() {
    let invalid: tonic::Status = ServiceError::Validation("Name required".into()).into();
    assert_eq!(invalid.code(), tonic::Code::InvalidArgument);
    let missing: tonic::Status = RepoError::NotFound.into();
    assert_eq!(missing.code(), tonic::Code::NotFound);
    let internal: tonic::Status =
        RepoError::database(std::io::Error::other("private diagnostic")).into();
    assert_eq!(internal.code(), tonic::Code::Internal);
    assert_eq!(internal.message(), "Database operation failed");
}
