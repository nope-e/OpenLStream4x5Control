use crate::ControlId;
use thiserror::Error;

/// Failures reported by a platform backend.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum BackendError {
    #[error("required device driver or vendor API is missing: {details}")]
    DriverMissing { details: String },
    #[error("permission denied: {details}")]
    PermissionDenied { details: String },
    #[error("device is busy: {details}")]
    Busy { details: String },
    #[error("device disconnected")]
    Disconnected,
    #[error("protocol mismatch: {details}")]
    ProtocolMismatch { details: String },
    #[error("invalid value for {control:?}: {reason}")]
    InvalidValue { control: ControlId, reason: String },
    #[error("unsupported operation: {feature}")]
    Unsupported { feature: String },
}

pub type BackendResult<T> = Result<T, BackendError>;
