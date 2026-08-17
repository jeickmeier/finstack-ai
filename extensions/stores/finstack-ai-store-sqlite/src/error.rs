use finstack_ai_protocol::ProtocolError;
use finstack_ai_runtime::StoreError;

pub(crate) fn i64_from_u64(value: u64, reason_code: &'static str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

pub(crate) fn u64_from_i64(value: i64, reason_code: &'static str) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

pub(crate) fn u16_from_i64(value: i64, reason_code: &'static str) -> Result<u16, StoreError> {
    u16::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

pub(crate) fn usize_from_i64(value: i64, reason_code: &'static str) -> Result<usize, StoreError> {
    usize::try_from(value).map_err(|_| StoreError::Integrity { reason_code })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "rusqlite map_err adapter takes the owned error"
)]
pub(crate) fn map_sqlite_error(error: rusqlite::Error) -> StoreError {
    if let Some(code) = error.sqlite_error_code() {
        return match code {
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => {
                StoreError::Unavailable {
                    reason_code: "sqlite_busy",
                }
            }
            rusqlite::ErrorCode::DiskFull => StoreError::Unavailable {
                reason_code: "sqlite_disk_full",
            },
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase => {
                StoreError::Integrity {
                    reason_code: "sqlite_corrupt",
                }
            }
            _ => StoreError::Unavailable {
                reason_code: "sqlite_error",
            },
        };
    }
    StoreError::Unavailable {
        reason_code: "sqlite_error",
    }
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "protocol map_err adapter takes the owned error"
)]
pub(crate) fn protocol_error(error: ProtocolError) -> StoreError {
    match error {
        ProtocolError::LimitExceeded { resource, limit } => {
            StoreError::LimitExceeded { resource, limit }
        }
        ProtocolError::Integrity { reason_code } | ProtocolError::InvalidCbor { reason_code } => {
            StoreError::Integrity { reason_code }
        }
        ProtocolError::Codec { .. } => StoreError::Integrity {
            reason_code: "canonical_codec",
        },
    }
}
