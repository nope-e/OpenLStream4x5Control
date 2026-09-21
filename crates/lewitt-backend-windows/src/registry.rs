use crate::VENDOR_API_CLSID;
use lewitt_core::{BackendError, BackendResult};
use std::io;
use std::path::{Path, PathBuf};
use winreg::RegKey;
use winreg::enums::{HKEY_CLASSES_ROOT, KEY_READ, KEY_WOW64_64KEY};

const VENDOR_API_FILE_NAME: &str = "dgtstreamapi_x64.dll";

/// Resolve the 64-bit `InprocServer32` registration and validate its target.
///
/// This function never searches the process directory and never loads the DLL.
pub fn locate_registered_vendor_api() -> BackendResult<PathBuf> {
    let classes = RegKey::predef(HKEY_CLASSES_ROOT);
    let key_path = format!(r"CLSID\{VENDOR_API_CLSID}\InprocServer32");
    let server = classes
        .open_subkey_with_flags(key_path, KEY_READ | KEY_WOW64_64KEY)
        .map_err(|error| map_registry_error(&error))?;
    let registered: String = server
        .get_value("")
        .map_err(|error| map_registry_error(&error))?;
    validate_registered_path(&registered)
}

fn validate_registered_path(registered: &str) -> BackendResult<PathBuf> {
    let registered = registered.trim();
    if registered.is_empty() {
        return Err(BackendError::DriverMissing {
            details: "registered InprocServer32 value is empty".into(),
        });
    }
    let path = Path::new(registered);
    if !path.is_absolute() {
        return Err(BackendError::ProtocolMismatch {
            details: "registered vendor API path is not absolute".into(),
        });
    }
    let file_name_matches = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(VENDOR_API_FILE_NAME));
    if !file_name_matches {
        return Err(BackendError::ProtocolMismatch {
            details: format!("registered vendor API is not {VENDOR_API_FILE_NAME}"),
        });
    }
    if !path.is_file() {
        return Err(BackendError::DriverMissing {
            details: "registered vendor API file does not exist".into(),
        });
    }
    path.canonicalize().map_err(|error| map_file_error(&error))
}

fn map_registry_error(error: &io::Error) -> BackendError {
    match error.kind() {
        io::ErrorKind::NotFound => BackendError::DriverMissing {
            details: format!("64-bit vendor API is not registered under {VENDOR_API_CLSID}"),
        },
        io::ErrorKind::PermissionDenied => BackendError::PermissionDenied {
            details: "cannot read the vendor API registry key".into(),
        },
        _ => BackendError::ProtocolMismatch {
            details: format!("cannot read vendor API registration: {error}"),
        },
    }
}

fn map_file_error(error: &io::Error) -> BackendError {
    match error.kind() {
        io::ErrorKind::NotFound => BackendError::DriverMissing {
            details: "registered vendor API file disappeared during validation".into(),
        },
        io::ErrorKind::PermissionDenied => BackendError::PermissionDenied {
            details: "cannot inspect the registered vendor API file".into(),
        },
        _ => BackendError::ProtocolMismatch {
            details: format!("cannot canonicalize registered vendor API path: {error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_is_rejected_instead_of_searching_current_directory() {
        assert!(matches!(
            validate_registered_path(VENDOR_API_FILE_NAME),
            Err(BackendError::ProtocolMismatch { .. })
        ));
    }

    #[test]
    fn unexpected_dll_name_is_rejected() {
        assert!(matches!(
            validate_registered_path(r"C:\Vendor\other.dll"),
            Err(BackendError::ProtocolMismatch { .. })
        ));
    }
}
