use crate::{
    BackendResult, ControlCommand, ControlId, ControlValue, DeviceCapabilities, DeviceEvent,
    DeviceId, DeviceInfo, DeviceSnapshot, MeterFrame,
};
use std::time::Duration;

/// Blocking device interface implemented by each platform backend.
///
/// A backend instance is moved to a [`crate::Controller`] worker. Callers must
/// not invoke it concurrently or expose native handles and callbacks through
/// this interface.
pub trait DeviceBackend: Send + 'static {
    fn enumerate(&mut self) -> BackendResult<Vec<DeviceInfo>>;
    fn open(&mut self, device: &DeviceId) -> BackendResult<()>;
    fn close(&mut self) -> BackendResult<()>;
    fn capabilities(&mut self) -> BackendResult<DeviceCapabilities>;
    fn read_snapshot(&mut self) -> BackendResult<DeviceSnapshot>;
    fn set_control(&mut self, command: &ControlCommand) -> BackendResult<()>;
    fn read_back(&mut self, control: &ControlId) -> BackendResult<ControlValue>;
    fn read_meters(&mut self) -> BackendResult<MeterFrame>;
    fn poll_events(&mut self, timeout: Duration) -> BackendResult<Vec<DeviceEvent>>;
}
