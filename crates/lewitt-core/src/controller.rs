use crate::{
    BackendError, ControlAccess, ControlCommand, ControlId, ControlKind, ControlValue,
    DeviceBackend, DeviceCapabilities, DeviceEvent, DeviceId, DeviceInfo, DeviceSnapshot,
    FirmwareStatus, MeterFrame,
};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, TrySendError, bounded};
use std::collections::BTreeMap;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub command_capacity: usize,
    pub event_capacity: usize,
    pub continuous_write_interval: Duration,
    pub visible_meter_interval: Duration,
    pub hidden_meter_interval: Duration,
    pub event_poll_interval: Duration,
    pub idle_wakeup_interval: Duration,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            command_capacity: 64,
            event_capacity: 64,
            continuous_write_interval: Duration::from_millis(33),
            visible_meter_interval: Duration::from_millis(33),
            hidden_meter_interval: Duration::from_millis(500),
            event_poll_interval: Duration::from_millis(250),
            idle_wakeup_interval: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerOperation {
    Enumerate,
    Connect,
    Disconnect,
    Capabilities,
    ReadSnapshot,
    SetControl,
    ReadBack,
    ReadMeters,
    PollEvents,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ControllerEvent {
    Devices(Vec<DeviceInfo>),
    Connected {
        device: DeviceInfo,
        capabilities: DeviceCapabilities,
        snapshot: DeviceSnapshot,
    },
    Disconnected,
    Snapshot(DeviceSnapshot),
    ControlConfirmed {
        control: ControlId,
        value: ControlValue,
    },
    Device(DeviceEvent),
    Error {
        operation: WorkerOperation,
        error: BackendError,
        snapshot: Option<DeviceSnapshot>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ControllerError {
    #[error("controller command queue is full")]
    QueueFull,
    #[error("controller worker has stopped")]
    Stopped,
    #[error("failed to spawn controller worker: {0}")]
    Spawn(String),
    #[error("controller worker panicked")]
    WorkerPanicked,
}

enum WorkerCommand {
    Enumerate,
    Connect(DeviceId),
    Disconnect,
    SetControl(ControlCommand),
    Refresh,
    SetVisible(bool),
    Shutdown,
}

/// Handle to the dedicated thread that exclusively owns a device backend.
pub struct Controller {
    commands: Sender<WorkerCommand>,
    events: Receiver<ControllerEvent>,
    meters: Receiver<MeterFrame>,
    worker: Option<JoinHandle<()>>,
}

impl Controller {
    pub fn spawn<B>(backend: B, config: ControllerConfig) -> Result<Self, ControllerError>
    where
        B: DeviceBackend,
    {
        let (command_tx, command_rx) = bounded(config.command_capacity);
        let (event_tx, event_rx) = bounded(config.event_capacity);
        let (meter_tx, meter_rx) = bounded(1);
        let stale_meter_rx = meter_rx.clone();
        let worker = thread::Builder::new()
            .name("lewitt-device-worker".into())
            .spawn(move || {
                Worker::new(
                    backend,
                    config,
                    command_rx,
                    event_tx,
                    meter_tx,
                    stale_meter_rx,
                )
                .run();
            })
            .map_err(|error| ControllerError::Spawn(error.to_string()))?;
        Ok(Self {
            commands: command_tx,
            events: event_rx,
            meters: meter_rx,
            worker: Some(worker),
        })
    }

    pub fn enumerate(&self) -> Result<(), ControllerError> {
        self.send(WorkerCommand::Enumerate)
    }

    pub fn connect(&self, device: DeviceId) -> Result<(), ControllerError> {
        self.send(WorkerCommand::Connect(device))
    }

    pub fn disconnect(&self) -> Result<(), ControllerError> {
        self.send(WorkerCommand::Disconnect)
    }

    pub fn set_control(&self, command: ControlCommand) -> Result<(), ControllerError> {
        self.send(WorkerCommand::SetControl(command))
    }

    pub fn refresh(&self) -> Result<(), ControllerError> {
        self.send(WorkerCommand::Refresh)
    }

    pub fn set_visible(&self, visible: bool) -> Result<(), ControllerError> {
        self.send(WorkerCommand::SetVisible(visible))
    }

    pub fn try_recv_event(&self) -> Result<Option<ControllerEvent>, ControllerError> {
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ControllerError::Stopped),
        }
    }

    pub fn recv_event_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Option<ControllerEvent>, ControllerError> {
        match self.events.recv_timeout(timeout) {
            Ok(event) => Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(ControllerError::Stopped),
        }
    }

    pub fn try_recv_meter(&self) -> Result<Option<MeterFrame>, ControllerError> {
        match self.meters.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ControllerError::Stopped),
        }
    }

    pub fn shutdown(mut self) -> Result<(), ControllerError> {
        self.stop()
    }

    fn send(&self, command: WorkerCommand) -> Result<(), ControllerError> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(ControllerError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(ControllerError::Stopped),
        }
    }

    fn stop(&mut self) -> Result<(), ControllerError> {
        if self.worker.is_none() {
            return Ok(());
        }
        let _ = self.commands.send(WorkerCommand::Shutdown);
        let worker = self.worker.take().ok_or(ControllerError::Stopped)?;
        worker.join().map_err(|_| ControllerError::WorkerPanicked)
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

struct Worker<B> {
    backend: B,
    config: ControllerConfig,
    commands: Receiver<WorkerCommand>,
    events: Sender<ControllerEvent>,
    meters: Sender<MeterFrame>,
    stale_meters: Receiver<MeterFrame>,
    capabilities: Option<DeviceCapabilities>,
    pending_continuous: BTreeMap<ControlId, ControlCommand>,
    connected: bool,
    visible: bool,
    next_flush: Instant,
    next_meter: Instant,
    next_event_poll: Instant,
}

impl<B> Worker<B>
where
    B: DeviceBackend,
{
    fn new(
        backend: B,
        config: ControllerConfig,
        commands: Receiver<WorkerCommand>,
        events: Sender<ControllerEvent>,
        meters: Sender<MeterFrame>,
        stale_meters: Receiver<MeterFrame>,
    ) -> Self {
        let now = Instant::now();
        Self {
            backend,
            next_flush: now + config.continuous_write_interval,
            next_meter: now + config.visible_meter_interval,
            next_event_poll: now + config.event_poll_interval,
            config,
            commands,
            events,
            meters,
            stale_meters,
            capabilities: None,
            pending_continuous: BTreeMap::new(),
            connected: false,
            visible: true,
        }
    }

    fn run(mut self) {
        loop {
            let timeout = self.wait_duration();
            match self.commands.recv_timeout(timeout) {
                Ok(command) => {
                    if self.handle_command(command) {
                        break;
                    }
                    while let Ok(command) = self.commands.try_recv() {
                        if self.handle_command(command) {
                            self.close_backend();
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.process_due_work();
        }
        self.close_backend();
    }

    fn wait_duration(&self) -> Duration {
        let now = Instant::now();
        [
            self.next_flush.saturating_duration_since(now),
            self.next_meter.saturating_duration_since(now),
            self.next_event_poll.saturating_duration_since(now),
            self.config.idle_wakeup_interval,
        ]
        .into_iter()
        .min()
        .unwrap_or(self.config.idle_wakeup_interval)
    }

    /// Returns true when the worker should stop.
    fn handle_command(&mut self, command: WorkerCommand) -> bool {
        match command {
            WorkerCommand::Enumerate => self.enumerate(),
            WorkerCommand::Connect(device) => self.connect(&device),
            WorkerCommand::Disconnect => self.disconnect(),
            WorkerCommand::SetControl(command) => self.queue_or_write(command),
            WorkerCommand::Refresh => self.refresh_snapshot(),
            WorkerCommand::SetVisible(visible) => {
                self.visible = visible;
                self.next_meter = Instant::now() + self.meter_interval();
            }
            WorkerCommand::Shutdown => return true,
        }
        false
    }

    fn enumerate(&mut self) {
        match self.backend.enumerate() {
            Ok(devices) => self.emit(ControllerEvent::Devices(devices)),
            Err(error) => self.emit_error(WorkerOperation::Enumerate, error, None),
        }
    }

    fn connect(&mut self, device_id: &DeviceId) {
        if self.connected {
            let _ = self.backend.close();
            self.connected = false;
            self.capabilities = None;
            self.pending_continuous.clear();
        }
        let device = match self.backend.enumerate() {
            Ok(devices) => devices.into_iter().find(|device| &device.id == device_id),
            Err(error) => {
                self.emit_error(WorkerOperation::Enumerate, error, None);
                return;
            }
        };
        let Some(device) = device else {
            self.emit_error(WorkerOperation::Connect, BackendError::Disconnected, None);
            return;
        };
        if !device.is_stream_4x5() {
            self.emit_error(
                WorkerOperation::Connect,
                BackendError::Unsupported {
                    feature: format!(
                        "USB device {:04X}:{:04X}",
                        device.vendor_id, device.product_id
                    ),
                },
                None,
            );
            return;
        }
        if let Err(error) = self.backend.open(device_id) {
            self.emit_error(WorkerOperation::Connect, error, None);
            return;
        }
        let capabilities = match self.backend.capabilities() {
            Ok(capabilities) => capabilities,
            Err(error) => {
                let _ = self.backend.close();
                self.emit_error(WorkerOperation::Capabilities, error, None);
                return;
            }
        };
        let snapshot = match self.backend.read_snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let _ = self.backend.close();
                self.emit_error(WorkerOperation::ReadSnapshot, error, None);
                return;
            }
        };
        self.connected = true;
        self.capabilities = Some(capabilities.clone());
        let now = Instant::now();
        self.next_flush = now + self.config.continuous_write_interval;
        self.next_meter = now + self.meter_interval();
        self.next_event_poll = now + self.config.event_poll_interval;
        self.emit(ControllerEvent::Connected {
            device,
            capabilities,
            snapshot,
        });
    }

    fn disconnect(&mut self) {
        self.pending_continuous.clear();
        self.capabilities = None;
        self.connected = false;
        match self.backend.close() {
            Ok(()) => self.emit(ControllerEvent::Disconnected),
            Err(error) => self.emit_error(WorkerOperation::Disconnect, error, None),
        }
    }

    fn queue_or_write(&mut self, command: ControlCommand) {
        let Some(capabilities) = &self.capabilities else {
            self.emit_error(
                WorkerOperation::SetControl,
                BackendError::Disconnected,
                None,
            );
            return;
        };
        if !capabilities.writes_enabled() {
            let version = match &capabilities.firmware {
                FirmwareStatus::Verified { .. } => None,
                FirmwareStatus::UnknownReadOnly { version }
                | FirmwareStatus::Unsupported { version } => version.clone(),
            };
            self.emit_error(
                WorkerOperation::SetControl,
                BackendError::UnsupportedFirmware { version },
                None,
            );
            return;
        }
        let Some(descriptor) = capabilities.descriptor(&command.control) else {
            self.emit_error(
                WorkerOperation::SetControl,
                BackendError::Unsupported {
                    feature: format!("control {:?}", command.control),
                },
                None,
            );
            return;
        };
        if descriptor.access != ControlAccess::Writable {
            self.emit_error(
                WorkerOperation::SetControl,
                BackendError::Unsupported {
                    feature: format!("write to read-only control {:?}", command.control),
                },
                None,
            );
            return;
        }
        if let Err(reason) = descriptor.validate(&command.value) {
            self.emit_error(
                WorkerOperation::SetControl,
                BackendError::InvalidValue {
                    control: command.control.clone(),
                    reason,
                },
                None,
            );
            return;
        }
        if descriptor.kind == ControlKind::Continuous {
            self.pending_continuous
                .insert(command.control.clone(), command);
        } else {
            self.write_and_read_back(command);
        }
    }

    fn write_and_read_back(&mut self, command: ControlCommand) {
        if let Err(error) = self.backend.set_control(&command) {
            self.handle_write_failure(WorkerOperation::SetControl, error);
            return;
        }
        match self.backend.read_back(&command.control) {
            Ok(value) => self.emit(ControllerEvent::ControlConfirmed {
                control: command.control,
                value,
            }),
            Err(error) => self.handle_write_failure(WorkerOperation::ReadBack, error),
        }
    }

    fn handle_write_failure(&mut self, operation: WorkerOperation, error: BackendError) {
        if error == BackendError::Disconnected {
            self.mark_disconnected();
            self.emit_error(operation, error, None);
            return;
        }
        let snapshot = self.backend.read_snapshot().ok();
        self.emit_error(operation, error, snapshot);
    }

    fn refresh_snapshot(&mut self) {
        if !self.connected {
            self.emit_error(
                WorkerOperation::ReadSnapshot,
                BackendError::Disconnected,
                None,
            );
            return;
        }
        match self.backend.read_snapshot() {
            Ok(snapshot) => self.emit(ControllerEvent::Snapshot(snapshot)),
            Err(error) => self.handle_runtime_error(WorkerOperation::ReadSnapshot, error),
        }
    }

    fn process_due_work(&mut self) {
        let now = Instant::now();
        if now >= self.next_flush {
            let pending = std::mem::take(&mut self.pending_continuous);
            for command in pending.into_values() {
                self.write_and_read_back(command);
            }
            self.next_flush = now + self.config.continuous_write_interval;
        }
        if now >= self.next_meter {
            let meters_available = self
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| !capabilities.meter_sources.is_empty());
            if self.connected && meters_available {
                match self.backend.read_meters() {
                    Ok(frame) => self.publish_latest_meter(frame),
                    Err(error) => self.handle_runtime_error(WorkerOperation::ReadMeters, error),
                }
            }
            self.next_meter = now + self.meter_interval();
        }
        if now >= self.next_event_poll {
            match self.backend.poll_events(Duration::ZERO) {
                Ok(events) => {
                    for event in events {
                        if event == DeviceEvent::Disconnected {
                            self.mark_disconnected();
                        }
                        self.emit(ControllerEvent::Device(event));
                    }
                }
                Err(error) => self.handle_runtime_error(WorkerOperation::PollEvents, error),
            }
            self.next_event_poll = now + self.config.event_poll_interval;
        }
    }

    fn publish_latest_meter(&self, frame: MeterFrame) {
        match self.meters.try_send(frame) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(frame)) => {
                let _ = self.stale_meters.try_recv();
                let _ = self.meters.try_send(frame);
            }
        }
    }

    fn handle_runtime_error(&mut self, operation: WorkerOperation, error: BackendError) {
        if error == BackendError::Disconnected {
            let was_connected = self.connected;
            self.mark_disconnected();
            if was_connected {
                self.emit_error(operation, error, None);
            }
            return;
        }
        self.emit_error(operation, error, None);
    }

    fn mark_disconnected(&mut self) {
        let was_connected = self.connected;
        self.connected = false;
        self.capabilities = None;
        self.pending_continuous.clear();
        if was_connected {
            self.emit(ControllerEvent::Disconnected);
        }
    }

    fn meter_interval(&self) -> Duration {
        if self.visible {
            self.config.visible_meter_interval
        } else {
            self.config.hidden_meter_interval
        }
    }

    fn emit(&self, event: ControllerEvent) {
        let _ = self.events.try_send(event);
    }

    fn emit_error(
        &self,
        operation: WorkerOperation,
        error: BackendError,
        snapshot: Option<DeviceSnapshot>,
    ) {
        self.emit(ControllerEvent::Error {
            operation,
            error,
            snapshot,
        });
    }

    fn close_backend(&mut self) {
        self.pending_continuous.clear();
        let _ = self.backend.close();
        self.connected = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockBackend, MockOperation};

    fn quiet_config() -> ControllerConfig {
        ControllerConfig {
            continuous_write_interval: Duration::from_millis(40),
            visible_meter_interval: Duration::from_secs(1),
            hidden_meter_interval: Duration::from_secs(1),
            event_poll_interval: Duration::from_secs(1),
            ..ControllerConfig::default()
        }
    }

    fn connect(controller: &Controller) {
        controller
            .connect(DeviceId::new("mock-stream-4x5"))
            .expect("connect command should enqueue");
        let event = controller
            .recv_event_timeout(Duration::from_secs(1))
            .expect("worker should be running")
            .expect("connect event should arrive");
        assert!(matches!(event, ControllerEvent::Connected { .. }));
    }

    fn recv_until(
        controller: &Controller,
        predicate: impl Fn(&ControllerEvent) -> bool,
    ) -> ControllerEvent {
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = controller
                .recv_event_timeout(remaining)
                .expect("worker should be running")
                .expect("matching event should arrive");
            if predicate(&event) {
                return event;
            }
        }
    }

    #[test]
    fn continuous_writes_are_coalesced() {
        let (backend, handle) = MockBackend::stream_4x5();
        let controller = Controller::spawn(backend, quiet_config()).expect("worker should spawn");
        connect(&controller);

        for value in [-20.0, -15.0, -10.0] {
            controller
                .set_control(ControlCommand {
                    control: ControlId::MonitorVolume,
                    value: ControlValue::Decibels(value),
                })
                .expect("control command should enqueue");
        }
        let event = controller
            .recv_event_timeout(Duration::from_secs(1))
            .expect("worker should be running")
            .expect("confirmation should arrive");
        assert!(matches!(
            event,
            ControllerEvent::ControlConfirmed {
                control: ControlId::MonitorVolume,
                value: ControlValue::Decibels(-10.0)
            }
        ));
        let writes = handle.take_writes().expect("writes should be observable");
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].command.value, ControlValue::Decibels(-10.0));
    }

    #[test]
    fn failed_read_back_refreshes_real_state() {
        let (backend, handle) = MockBackend::stream_4x5();
        let controller = Controller::spawn(backend, quiet_config()).expect("worker should spawn");
        connect(&controller);
        handle
            .fail_next(
                MockOperation::ReadBack,
                BackendError::ProtocolMismatch {
                    details: "injected read-back failure".into(),
                },
            )
            .expect("failure should be injectable");
        controller
            .set_control(ControlCommand {
                control: ControlId::DuckerEnabled,
                value: ControlValue::Boolean(true),
            })
            .expect("control command should enqueue");

        let event = controller
            .recv_event_timeout(Duration::from_secs(1))
            .expect("worker should be running")
            .expect("error should arrive");
        let ControllerEvent::Error {
            operation,
            snapshot: Some(snapshot),
            ..
        } = event
        else {
            panic!("expected an error with refreshed state");
        };
        assert_eq!(operation, WorkerOperation::ReadBack);
        assert_eq!(
            snapshot.controls.get(&ControlId::DuckerEnabled),
            Some(&ControlValue::Boolean(true))
        );
    }

    #[test]
    fn disconnect_during_write_stops_io_and_can_reconnect() {
        let (backend, handle) = MockBackend::stream_4x5();
        let controller = Controller::spawn(backend, quiet_config()).expect("worker should spawn");
        connect(&controller);
        handle
            .disconnect()
            .expect("disconnect should be injectable");

        controller
            .set_control(ControlCommand {
                control: ControlId::DuckerEnabled,
                value: ControlValue::Boolean(true),
            })
            .expect("control command should enqueue");
        let disconnected = recv_until(&controller, |event| {
            matches!(event, ControllerEvent::Disconnected)
        });
        assert_eq!(disconnected, ControllerEvent::Disconnected);
        let error = recv_until(&controller, |event| {
            matches!(
                event,
                ControllerEvent::Error {
                    operation: WorkerOperation::SetControl,
                    error: BackendError::Disconnected,
                    snapshot: None
                }
            )
        });
        assert!(matches!(error, ControllerEvent::Error { .. }));
        assert!(
            handle
                .take_writes()
                .expect("writes should be observable")
                .is_empty()
        );

        handle.reconnect().expect("reconnect should be injectable");
        controller
            .connect(DeviceId::new("mock-stream-4x5"))
            .expect("reconnect command should enqueue");
        let reconnected = recv_until(&controller, |event| {
            matches!(event, ControllerEvent::Connected { .. })
        });
        assert!(matches!(reconnected, ControllerEvent::Connected { .. }));
    }

    #[test]
    fn unknown_firmware_is_rejected_before_backend_write() {
        let (backend, handle) = MockBackend::stream_4x5();
        handle
            .set_firmware_status(FirmwareStatus::UnknownReadOnly {
                version: Some("future".into()),
            })
            .expect("firmware state should be injectable");
        let controller = Controller::spawn(backend, quiet_config()).expect("worker should spawn");
        connect(&controller);

        controller
            .set_control(ControlCommand {
                control: ControlId::DuckerEnabled,
                value: ControlValue::Boolean(true),
            })
            .expect("control command should enqueue");
        let event = recv_until(&controller, |event| {
            matches!(
                event,
                ControllerEvent::Error {
                    error: BackendError::UnsupportedFirmware { .. },
                    ..
                }
            )
        });
        assert!(matches!(event, ControllerEvent::Error { .. }));
        assert!(
            handle
                .take_writes()
                .expect("writes should be observable")
                .is_empty()
        );
    }

    #[test]
    fn meter_queue_keeps_only_latest_frame() {
        let (backend, _) = MockBackend::stream_4x5();
        let config = ControllerConfig {
            visible_meter_interval: Duration::from_millis(5),
            hidden_meter_interval: Duration::from_secs(1),
            event_poll_interval: Duration::from_secs(1),
            ..quiet_config()
        };
        let controller = Controller::spawn(backend, config).expect("worker should spawn");
        connect(&controller);
        thread::sleep(Duration::from_millis(40));
        controller
            .set_visible(false)
            .expect("visibility command should enqueue");
        thread::sleep(Duration::from_millis(20));

        let frame = controller
            .try_recv_meter()
            .expect("worker should be running")
            .expect("latest meter should be retained");
        assert!(frame.sequence > 1);
        assert_eq!(
            controller
                .try_recv_meter()
                .expect("worker should be running"),
            None
        );
    }
}
