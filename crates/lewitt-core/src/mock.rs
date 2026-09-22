use crate::{
    BackendError, BackendResult, BusId, ChannelId, ControlCommand, ControlDescriptor, ControlId,
    ControlType, ControlValue, DeviceBackend, DeviceCapabilities, DeviceEvent, DeviceId,
    DeviceInfo, DeviceSnapshot, MeterFrame, STREAM_4X5_PRODUCT_ID, STREAM_4X5_VENDOR_ID,
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MockOperation {
    Enumerate,
    Open,
    Close,
    Capabilities,
    ReadSnapshot,
    SetControl,
    ReadBack,
    ReadMeters,
    PollEvents,
}

#[derive(Debug, Clone)]
pub struct MockWrite {
    pub command: ControlCommand,
    pub at: Instant,
}

#[derive(Debug)]
struct MockState {
    device: DeviceInfo,
    capabilities: DeviceCapabilities,
    snapshot: DeviceSnapshot,
    meter_frame: MeterFrame,
    events: VecDeque<DeviceEvent>,
    failures: VecDeque<(MockOperation, BackendError)>,
    writes: Vec<MockWrite>,
    connected: bool,
    open: bool,
}

/// Test-side access to a [`MockBackend`] after it has moved to a worker thread.
#[derive(Debug, Clone)]
pub struct MockHandle {
    state: Arc<Mutex<MockState>>,
}

impl MockHandle {
    pub fn fail_next(&self, operation: MockOperation, error: BackendError) -> BackendResult<()> {
        self.lock()?.failures.push_back((operation, error));
        Ok(())
    }

    pub fn disconnect(&self) -> BackendResult<()> {
        let mut state = self.lock()?;
        state.connected = false;
        state.open = false;
        state.events.push_back(DeviceEvent::Disconnected);
        Ok(())
    }

    pub fn reconnect(&self) -> BackendResult<()> {
        let mut state = self.lock()?;
        state.connected = true;
        let device = state.device.clone();
        state.events.push_back(DeviceEvent::Connected { device });
        Ok(())
    }

    pub fn set_writable(&self, control: &ControlId, writable: bool) -> BackendResult<()> {
        let mut state = self.lock()?;
        let descriptor = state
            .capabilities
            .controls
            .iter_mut()
            .find(|descriptor| &descriptor.id == control)
            .ok_or_else(|| BackendError::Unsupported {
                feature: format!("mock capability {control:?}"),
            })?;
        descriptor.writable = writable;
        Ok(())
    }

    pub fn set_meter_frame(&self, frame: MeterFrame) -> BackendResult<()> {
        self.lock()?.meter_frame = frame;
        Ok(())
    }

    pub fn set_control_value(&self, control: ControlId, value: ControlValue) -> BackendResult<()> {
        let mut state = self.lock()?;
        state.snapshot.controls.insert(control, value);
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        Ok(())
    }

    pub fn snapshot(&self) -> BackendResult<DeviceSnapshot> {
        Ok(self.lock()?.snapshot.clone())
    }

    pub fn take_writes(&self) -> BackendResult<Vec<MockWrite>> {
        Ok(std::mem::take(&mut self.lock()?.writes))
    }

    fn lock(&self) -> BackendResult<MutexGuard<'_, MockState>> {
        self.state
            .lock()
            .map_err(|_| BackendError::ProtocolMismatch {
                details: "mock state lock was poisoned".into(),
            })
    }
}

/// Hardware-free backend with observable state and injectable failures.
pub struct MockBackend {
    state: Arc<Mutex<MockState>>,
}

impl MockBackend {
    #[must_use]
    pub fn stream_4x5() -> (Self, MockHandle) {
        let monitor = ControlId::MonitorVolume;
        let ducker = ControlId::DuckerEnabled;
        let capabilities = DeviceCapabilities {
            model: "Stream 4x5 (mock)".into(),
            controls: vec![
                ControlDescriptor {
                    id: monitor.clone(),
                    value: ControlType::Decibels {
                        minimum: -60.0,
                        maximum: 12.0,
                        step: 0.5,
                    },
                    writable: true,
                },
                ControlDescriptor {
                    id: ducker.clone(),
                    value: ControlType::Boolean,
                    writable: true,
                },
            ],
            meter_sources: vec![
                crate::MeterId::Input {
                    channel: ChannelId::Input1,
                },
                crate::MeterId::Output {
                    bus: BusId::Output1_2,
                },
            ],
        };
        let snapshot = DeviceSnapshot {
            revision: 1,
            controls: [
                (monitor, ControlValue::Decibels(-12.0)),
                (ducker, ControlValue::Boolean(false)),
            ]
            .into_iter()
            .collect(),
        };
        let state = Arc::new(Mutex::new(MockState {
            device: DeviceInfo {
                id: DeviceId::new("mock-stream-4x5"),
                display_name: "Stream 4x5 (mock)".into(),
                vendor_id: STREAM_4X5_VENDOR_ID,
                product_id: STREAM_4X5_PRODUCT_ID,
                firmware_version: Some("mock".into()),
            },
            capabilities,
            snapshot,
            meter_frame: MeterFrame::default(),
            events: VecDeque::new(),
            failures: VecDeque::new(),
            writes: Vec::new(),
            connected: true,
            open: false,
        }));
        (
            Self {
                state: Arc::clone(&state),
            },
            MockHandle { state },
        )
    }

    fn lock(&self) -> BackendResult<MutexGuard<'_, MockState>> {
        self.state
            .lock()
            .map_err(|_| BackendError::ProtocolMismatch {
                details: "mock state lock was poisoned".into(),
            })
    }

    fn fail_if_requested(state: &mut MockState, operation: MockOperation) -> BackendResult<()> {
        let position = state
            .failures
            .iter()
            .position(|(candidate, _)| *candidate == operation);
        if let Some(position) = position
            && let Some((_, error)) = state.failures.remove(position)
        {
            return Err(error);
        }
        Ok(())
    }

    fn ensure_open(state: &MockState) -> BackendResult<()> {
        if !state.connected || !state.open {
            return Err(BackendError::Disconnected);
        }
        Ok(())
    }
}

impl DeviceBackend for MockBackend {
    fn enumerate(&mut self) -> BackendResult<Vec<DeviceInfo>> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::Enumerate)?;
        Ok(state
            .connected
            .then(|| state.device.clone())
            .into_iter()
            .collect())
    }

    fn open(&mut self, device: &DeviceId) -> BackendResult<()> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::Open)?;
        if !state.connected || device != &state.device.id {
            return Err(BackendError::Disconnected);
        }
        state.open = true;
        Ok(())
    }

    fn close(&mut self) -> BackendResult<()> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::Close)?;
        state.open = false;
        Ok(())
    }

    fn capabilities(&mut self) -> BackendResult<DeviceCapabilities> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::Capabilities)?;
        Self::ensure_open(&state)?;
        Ok(state.capabilities.clone())
    }

    fn read_snapshot(&mut self) -> BackendResult<DeviceSnapshot> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::ReadSnapshot)?;
        Self::ensure_open(&state)?;
        Ok(state.snapshot.clone())
    }

    fn set_control(&mut self, command: &ControlCommand) -> BackendResult<()> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::SetControl)?;
        Self::ensure_open(&state)?;
        let descriptor = state
            .capabilities
            .descriptor(&command.control)
            .ok_or_else(|| BackendError::Unsupported {
                feature: format!("control {:?}", command.control),
            })?;
        if !descriptor.writable {
            return Err(BackendError::Unsupported {
                feature: format!("write to read-only control {:?}", command.control),
            });
        }
        descriptor
            .validate(&command.value)
            .map_err(|reason| BackendError::InvalidValue {
                control: command.control.clone(),
                reason,
            })?;
        state
            .snapshot
            .controls
            .insert(command.control.clone(), command.value.clone());
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.writes.push(MockWrite {
            command: command.clone(),
            at: Instant::now(),
        });
        Ok(())
    }

    fn read_back(&mut self, control: &ControlId) -> BackendResult<ControlValue> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::ReadBack)?;
        Self::ensure_open(&state)?;
        state
            .snapshot
            .controls
            .get(control)
            .cloned()
            .ok_or_else(|| BackendError::Unsupported {
                feature: format!("read-back for {control:?}"),
            })
    }

    fn read_meters(&mut self) -> BackendResult<MeterFrame> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::ReadMeters)?;
        Self::ensure_open(&state)?;
        state.meter_frame.sequence = state.meter_frame.sequence.saturating_add(1);
        Ok(state.meter_frame.clone())
    }

    fn poll_events(&mut self, _timeout: Duration) -> BackendResult<Vec<DeviceEvent>> {
        let mut state = self.lock()?;
        Self::fail_if_requested(&mut state, MockOperation::PollEvents)?;
        Ok(state.events.drain(..).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_control_rejects_writes() {
        let (mut backend, handle) = MockBackend::stream_4x5();
        handle
            .set_writable(&ControlId::DuckerEnabled, false)
            .expect("mock state should be available");
        backend
            .open(&DeviceId::new("mock-stream-4x5"))
            .expect("mock device should open");

        let error = backend
            .set_control(&ControlCommand {
                control: ControlId::DuckerEnabled,
                value: ControlValue::Boolean(true),
            })
            .expect_err("unknown firmware must reject writes");
        assert!(matches!(error, BackendError::Unsupported { .. }));
    }
}
