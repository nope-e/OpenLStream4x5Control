//! Linux system-libusb backend boundary.
//!
//! No USB operation has a sanitized golden fixture yet, so this implementation
//! deliberately rejects device access and never claims or detaches an audio
//! interface.

use lewitt_core::{
    BackendError, BackendResult, ControlCommand, ControlId, ControlValue, DeviceBackend,
    DeviceCapabilities, DeviceEvent, DeviceId, DeviceInfo, DeviceSnapshot, MeterFrame,
};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct LinuxBackend;

impl LinuxBackend {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    fn unavailable<T>() -> BackendResult<T> {
        Err(BackendError::Unsupported {
            feature: "Linux USB protocol has no confirmed golden fixture".into(),
        })
    }
}

impl DeviceBackend for LinuxBackend {
    fn enumerate(&mut self) -> BackendResult<Vec<DeviceInfo>> {
        Self::unavailable()
    }

    fn open(&mut self, _device: &DeviceId) -> BackendResult<()> {
        Self::unavailable()
    }

    fn close(&mut self) -> BackendResult<()> {
        Ok(())
    }

    fn capabilities(&mut self) -> BackendResult<DeviceCapabilities> {
        Self::unavailable()
    }

    fn read_snapshot(&mut self) -> BackendResult<DeviceSnapshot> {
        Self::unavailable()
    }

    fn set_control(&mut self, _command: &ControlCommand) -> BackendResult<()> {
        Self::unavailable()
    }

    fn read_back(&mut self, _control: &ControlId) -> BackendResult<ControlValue> {
        Self::unavailable()
    }

    fn read_meters(&mut self) -> BackendResult<MeterFrame> {
        Self::unavailable()
    }

    fn poll_events(&mut self, _timeout: Duration) -> BackendResult<Vec<DeviceEvent>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unverified_protocol_never_enumerates_or_writes() {
        let mut backend = LinuxBackend::new();
        assert!(matches!(
            backend.enumerate(),
            Err(BackendError::Unsupported { .. })
        ));
        assert!(matches!(
            backend.set_control(&ControlCommand {
                control: lewitt_core::ControlId::DuckerEnabled,
                value: ControlValue::Boolean(true),
            }),
            Err(BackendError::Unsupported { .. })
        ));
    }
}
