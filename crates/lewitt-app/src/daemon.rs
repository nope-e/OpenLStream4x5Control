use iced::theme::Palette;
use iced::widget::{button, column, container, progress_bar, row, scrollable, slider, space, text};
use iced::{
    Alignment, Background, Color, Element, Fill, Length, Size, Subscription, Task, Theme, border,
    window,
};
use lewitt_core::{
    BackendError, BusId, ChannelId, ControlAccess, ControlCommand, ControlId, ControlValue,
    Controller, ControllerConfig, ControllerEvent, DeviceCapabilities, DeviceEvent, DeviceInfo,
    DeviceSnapshot, WorkerOperation,
};
use std::time::{Duration, Instant};

use crate::lifecycle::{Lifecycle, LifecycleAction, LifecycleEffect};
use crate::localization::{Catalog, Language};
use crate::single_instance::{InstanceCommand, PrimaryInstance};
#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::tray::Tray;

const WINDOW_SIZE: Size = Size::new(1040.0, 760.0);
const MIN_WINDOW_SIZE: Size = Size::new(780.0, 620.0);
const CONTROLLER_TICK: Duration = Duration::from_micros(16_667);
const VISIBLE_SNAPSHOT_INTERVAL: Duration = Duration::from_micros(16_667);
const HIDDEN_SNAPSHOT_INTERVAL: Duration = Duration::from_secs(5);

const BACKGROUND: Color = Color::from_rgb(0.035, 0.043, 0.055);
const SURFACE: Color = Color::from_rgb(0.066, 0.078, 0.098);
const SURFACE_RAISED: Color = Color::from_rgb(0.086, 0.102, 0.125);
const BORDER: Color = Color::from_rgb(0.16, 0.19, 0.23);
const TEXT_MUTED: Color = Color::from_rgb(0.58, 0.63, 0.70);
const ACCENT: Color = Color::from_rgb(0.20, 0.78, 0.66);
const WARNING: Color = Color::from_rgb(0.96, 0.68, 0.28);
const DANGER: Color = Color::from_rgb(0.92, 0.35, 0.38);

/// Runs the GUI as a daemon that can remain alive with no open windows.
pub fn run(background: bool, instance: PrimaryInstance) -> iced::Result {
    iced::daemon(move || boot(background, instance.clone()), update, view)
        .title(|state: &State, _window| state.text("app.title").to_owned())
        .subscription(subscription)
        .theme(|_state: &State, _window: window::Id| application_theme())
        .run()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DevicePhase {
    Discovering,
    Connecting,
    Connected,
    NoDevice,
    Error,
}

struct State {
    lifecycle: Lifecycle,
    window: Option<window::Id>,
    language: Language,
    catalog: Catalog,
    runtime_notice: Option<String>,
    instance: Option<PrimaryInstance>,
    controller: Option<Controller>,
    device_phase: DevicePhase,
    device: Option<DeviceInfo>,
    capabilities: Option<DeviceCapabilities>,
    snapshot: DeviceSnapshot,
    next_snapshot_refresh: Instant,
    snapshot_refresh_pending: bool,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    tray: Option<Tray>,
}

impl State {
    fn new(background: bool, instance: Option<PrimaryInstance>) -> Self {
        let mut state = Self::without_controller(background, instance);
        state.start_controller();
        state
    }

    fn without_controller(background: bool, instance: Option<PrimaryInstance>) -> Self {
        let language =
            std::env::var("LANG").map_or(Language::English, |tag| Language::from_locale_tag(&tag));

        Self {
            lifecycle: Lifecycle::new(background),
            window: None,
            language,
            catalog: Catalog::for_language(language),
            runtime_notice: None,
            instance,
            controller: None,
            device_phase: DevicePhase::Discovering,
            device: None,
            capabilities: None,
            snapshot: DeviceSnapshot::default(),
            next_snapshot_refresh: Instant::now() + VISIBLE_SNAPSHOT_INTERVAL,
            snapshot_refresh_pending: false,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            tray: None,
        }
    }

    fn start_controller(&mut self) {
        match spawn_platform_controller() {
            Ok(controller) => {
                if let Err(error) = controller.enumerate() {
                    self.device_phase = DevicePhase::Error;
                    self.report_internal_error("initial device enumeration", &error);
                }
                self.controller = Some(controller);
            }
            Err(error) => {
                self.device_phase = DevicePhase::Error;
                self.report_internal_error("controller startup", &error);
            }
        }
    }

    fn text(&self, key: &'static str) -> &'static str {
        self.catalog.get(key).unwrap_or(key)
    }

    fn show_window(&mut self) -> Task<Message> {
        if let Some(controller) = &self.controller
            && let Err(error) = controller.set_visible(true)
        {
            self.report_internal_error("raising the meter polling rate", &error);
        }
        self.next_snapshot_refresh = Instant::now();
        if let Some(id) = self.window {
            return restore_and_raise_window(id);
        }

        let (id, opened) = window::open(window_settings());
        self.window = Some(id);
        opened.map(Message::WindowOpened)
    }

    fn initialize_tray(&mut self) -> bool {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        match Tray::new(&self.catalog) {
            Ok(tray) => {
                self.tray = Some(tray);
                true
            }
            Err(error) => {
                tracing::error!(error = %error, "failed to initialize the system tray");
                self.runtime_notice = Some(self.text("notice.tray_failed").to_owned());
                false
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            tracing::warn!("system tray is unsupported on this platform");
            self.runtime_notice = Some(self.text("notice.tray_unsupported").to_owned());
            false
        }
    }

    fn set_language(&mut self, language: Language) {
        self.language = language;
        self.catalog = Catalog::for_language(language);

        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if let Some(tray) = &self.tray
            && let Err(error) = tray.set_language(&self.catalog)
        {
            tracing::error!(error = %error, "failed to update the system tray language");
            self.runtime_notice = Some(self.text("notice.tray_failed").to_owned());
        }
    }

    fn reconnect(&mut self) {
        self.device_phase = DevicePhase::Discovering;
        self.device = None;
        self.capabilities = None;
        self.snapshot = DeviceSnapshot::default();
        self.snapshot_refresh_pending = false;
        self.runtime_notice = None;
        if let Some(controller) = &self.controller {
            let disconnect_error = controller.disconnect().err();
            let enumerate_error = controller.enumerate().err();
            if let Some(error) = disconnect_error {
                self.report_internal_error("disconnect before reconnecting", &error);
            }
            if let Some(error) = enumerate_error {
                self.device_phase = DevicePhase::Error;
                self.report_internal_error("device enumeration during reconnect", &error);
            }
        }
    }

    fn refresh(&mut self) {
        if self.snapshot_refresh_pending {
            return;
        }
        if let Some(controller) = &self.controller {
            match controller.refresh() {
                Ok(()) => self.snapshot_refresh_pending = true,
                Err(error) => self.report_internal_error("snapshot refresh", &error),
            }
        }
    }

    fn apply(&mut self, action: LifecycleAction) -> Task<Message> {
        match self.lifecycle.apply(action) {
            Some(LifecycleEffect::HideWindow) => {
                if let Some(controller) = &self.controller
                    && let Err(error) = controller.set_visible(false)
                {
                    self.report_internal_error("lowering the meter polling rate", &error);
                }
                if let Some(id) = self.window.take() {
                    self.next_snapshot_refresh = Instant::now() + HIDDEN_SNAPSHOT_INTERVAL;
                    window::close(id)
                } else {
                    Task::none()
                }
            }
            Some(LifecycleEffect::ShowWindow) => self.show_window(),
            Some(LifecycleEffect::ShowStatus) => {
                self.refresh();
                self.show_window()
            }
            Some(LifecycleEffect::Reconnect) => {
                self.reconnect();
                Task::none()
            }
            Some(LifecycleEffect::Shutdown) => {
                if let Some(controller) = self.controller.take()
                    && let Err(error) = controller.shutdown()
                {
                    tracing::error!(error = %error, "controller shutdown failed");
                }
                tracing::info!("application shutdown requested");
                iced::exit()
            }
            None => Task::none(),
        }
    }

    fn poll_controller(&mut self) {
        let mut events = Vec::new();
        let mut receive_error = None;
        let mut meter_receive_error = None;
        if let Some(controller) = &self.controller {
            loop {
                match controller.try_recv_event() {
                    Ok(Some(event)) => events.push(event),
                    Ok(None) => break,
                    Err(error) => {
                        receive_error = Some(error);
                        break;
                    }
                }
            }
            loop {
                match controller.try_recv_meter() {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => {
                        meter_receive_error = Some(error);
                        break;
                    }
                }
            }
        }
        if let Some(error) = receive_error {
            self.device_phase = DevicePhase::Error;
            self.report_internal_error("controller event receive", &error);
        }
        if let Some(error) = meter_receive_error {
            self.report_internal_error("meter frame receive", &error);
        }

        for event in events {
            self.apply_controller_event(event);
        }

        if self.device_phase == DevicePhase::Connected
            && !self.snapshot_refresh_pending
            && Instant::now() >= self.next_snapshot_refresh
        {
            self.refresh();
            let interval = if self.lifecycle.window_visible() {
                VISIBLE_SNAPSHOT_INTERVAL
            } else {
                HIDDEN_SNAPSHOT_INTERVAL
            };
            self.next_snapshot_refresh = Instant::now() + interval;
        }
    }

    fn apply_controller_event(&mut self, event: ControllerEvent) {
        match event {
            ControllerEvent::Devices(devices) => {
                if let Some(device) = devices.into_iter().next() {
                    self.device_phase = DevicePhase::Connecting;
                    self.device = Some(device.clone());
                    if let Some(controller) = &self.controller
                        && let Err(error) = controller.connect(device.id)
                    {
                        self.device_phase = DevicePhase::Error;
                        self.report_internal_error("device connection", &error);
                    }
                } else {
                    self.device_phase = DevicePhase::NoDevice;
                    self.device = None;
                    self.capabilities = None;
                    self.snapshot = DeviceSnapshot::default();
                    self.snapshot_refresh_pending = false;
                }
            }
            ControllerEvent::Connected {
                device,
                capabilities,
                snapshot,
            } => {
                tracing::info!(
                    vendor_id = format_args!("{:#06X}", device.vendor_id),
                    product_id = format_args!("{:#06X}", device.product_id),
                    "connected to Stream 4x5"
                );
                self.device_phase = DevicePhase::Connected;
                self.device = Some(device);
                self.capabilities = Some(capabilities);
                self.snapshot = snapshot;
                self.snapshot_refresh_pending = false;
                self.runtime_notice = None;
                let interval = if self.lifecycle.window_visible() {
                    VISIBLE_SNAPSHOT_INTERVAL
                } else {
                    HIDDEN_SNAPSHOT_INTERVAL
                };
                self.next_snapshot_refresh = Instant::now() + interval;
            }
            ControllerEvent::Disconnected => {
                tracing::warn!("device disconnected");
                self.device_phase = DevicePhase::NoDevice;
                self.device = None;
                self.capabilities = None;
                self.snapshot = DeviceSnapshot::default();
                self.snapshot_refresh_pending = false;
            }
            ControllerEvent::Snapshot(snapshot) => {
                self.snapshot = snapshot;
                self.snapshot_refresh_pending = false;
            }
            ControllerEvent::ControlConfirmed { control, value } => {
                self.snapshot.controls.insert(control, value);
                self.snapshot.revision = self.snapshot.revision.saturating_add(1);
            }
            ControllerEvent::Device(DeviceEvent::Disconnected) => {
                self.device_phase = DevicePhase::NoDevice;
                self.snapshot_refresh_pending = false;
            }
            ControllerEvent::Device(
                DeviceEvent::StateChanged { .. } | DeviceEvent::Connected { .. },
            ) => self.refresh(),
            ControllerEvent::Device(DeviceEvent::ControlCenterConflict) => {
                tracing::warn!("Lewitt Control Center conflict reported by the backend");
                self.runtime_notice = Some(self.text("notice.control_center_conflict").to_owned());
            }
            ControllerEvent::Error {
                operation,
                error,
                snapshot,
            } => {
                if operation == WorkerOperation::ReadSnapshot {
                    self.snapshot_refresh_pending = false;
                }
                if let Some(snapshot) = snapshot {
                    self.snapshot = snapshot;
                }
                if matches!(
                    operation,
                    WorkerOperation::Enumerate
                        | WorkerOperation::Connect
                        | WorkerOperation::Capabilities
                        | WorkerOperation::ReadSnapshot
                ) && self.snapshot.controls.is_empty()
                {
                    self.device_phase = DevicePhase::Error;
                }
                self.report_backend_error(operation, &error);
            }
        }
    }

    fn report_internal_error(&mut self, context: &'static str, error: &impl std::fmt::Display) {
        tracing::error!(context, error = %error, "application operation failed");
        self.runtime_notice = Some(self.text("notice.controller_failed").to_owned());
    }

    fn report_backend_error(&mut self, operation: WorkerOperation, error: &BackendError) {
        tracing::error!(?operation, error = %error, "device backend operation failed");
        self.runtime_notice = Some(self.text(backend_notice_key(error)).to_owned());
    }

    fn phase_text(&self) -> &'static str {
        match self.device_phase {
            DevicePhase::Discovering => self.text("status.discovering"),
            DevicePhase::Connecting => self.text("status.connecting"),
            DevicePhase::Connected => self.text("status.connected"),
            DevicePhase::NoDevice => self.text("status.no_device"),
            DevicePhase::Error => self.text("status.error"),
        }
    }

    fn decibels(&self, control: &ControlId) -> Option<f32> {
        match self.snapshot.controls.get(control) {
            Some(ControlValue::Decibels(value)) => Some(*value),
            _ => None,
        }
    }

    fn boolean(&self, control: &ControlId) -> Option<bool> {
        match self.snapshot.controls.get(control) {
            Some(ControlValue::Boolean(value)) => Some(*value),
            _ => None,
        }
    }

    fn supports_control(&self, control: &ControlId) -> bool {
        self.capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.descriptor(control))
            .is_some()
    }

    fn can_write(&self, control: &ControlId) -> bool {
        self.capabilities.as_ref().is_some_and(|capabilities| {
            capabilities.writes_enabled()
                && capabilities
                    .descriptor(control)
                    .is_some_and(|descriptor| descriptor.access == ControlAccess::Writable)
        })
    }

    fn queue_control_write(&mut self, control: ControlId, value: ControlValue) {
        if !self.can_write(&control) {
            self.runtime_notice = Some(self.text("notice.operation_unavailable").to_owned());
            return;
        }
        let result = self.controller.as_ref().map_or_else(
            || Err(lewitt_core::ControllerError::Stopped),
            |controller| {
                controller.set_control(ControlCommand {
                    control: control.clone(),
                    value: value.clone(),
                })
            },
        );
        match result {
            Ok(()) => {
                self.snapshot.controls.insert(control, value);
            }
            Err(error) => self.report_internal_error("queueing a control write", &error),
        }
    }

    fn set_input_gain(&mut self, channel: ChannelId, gain_db: f32) {
        self.queue_control_write(
            ControlId::InputVolume { channel },
            ControlValue::Decibels(gain_db),
        );
    }

    fn set_high_pass(&mut self, channel: ChannelId, enabled: bool) {
        self.queue_control_write(
            ControlId::HighPass { channel },
            ControlValue::Boolean(enabled),
        );
    }

    fn set_phase_invert(&mut self, channel: ChannelId, enabled: bool) {
        self.queue_control_write(
            ControlId::PhaseInvert { channel },
            ControlValue::Boolean(enabled),
        );
    }

    fn set_phantom_power(&mut self, channel: ChannelId, enabled: bool) {
        self.queue_control_write(
            ControlId::PhantomPower { channel },
            ControlValue::Boolean(enabled),
        );
    }

    fn integer(&self, control: &ControlId) -> Option<i32> {
        match self.snapshot.controls.get(control) {
            Some(ControlValue::Integer(value)) => Some(*value),
            _ => None,
        }
    }

    fn choice(&self, control: &ControlId) -> Option<&str> {
        match self.snapshot.controls.get(control) {
            Some(ControlValue::Choice(value)) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Message {
    Action(LifecycleAction),
    Instance(InstanceCommand),
    SetLanguage(Language),
    Refresh,
    SetInputGain(ChannelId, f32),
    SetHighPass(ChannelId, bool),
    SetPhaseInvert(ChannelId, bool),
    SetPhantomPower(ChannelId, bool),
    ControllerTick,
    WindowCloseRequested(window::Id),
    WindowOpened(window::Id),
}

fn boot(background: bool, instance: PrimaryInstance) -> (State, Task<Message>) {
    let mut state = State::new(background, Some(instance));
    let tray_ready = state.initialize_tray();
    let task = if state.lifecycle.window_visible() || !tray_ready {
        if !state.lifecycle.window_visible() {
            let _ = state.lifecycle.apply(LifecycleAction::TrayShow);
        }
        state.show_window()
    } else {
        Task::none()
    };
    (state, task)
}

fn update(state: &mut State, message: Message) -> Task<Message> {
    match message {
        Message::Action(action) => state.apply(action),
        Message::Instance(InstanceCommand::ShowWindow) => {
            state.apply(LifecycleAction::SecondInstanceShow)
        }
        Message::SetLanguage(language) => {
            state.set_language(language);
            Task::none()
        }
        Message::Refresh => {
            state.refresh();
            Task::none()
        }
        Message::SetInputGain(channel, gain_db) => {
            state.set_input_gain(channel, gain_db);
            Task::none()
        }
        Message::SetHighPass(channel, enabled) => {
            state.set_high_pass(channel, enabled);
            Task::none()
        }
        Message::SetPhaseInvert(channel, enabled) => {
            state.set_phase_invert(channel, enabled);
            Task::none()
        }
        Message::SetPhantomPower(channel, enabled) => {
            state.set_phantom_power(channel, enabled);
            Task::none()
        }
        Message::ControllerTick => {
            state.poll_controller();
            Task::none()
        }
        Message::WindowCloseRequested(id) if state.window == Some(id) => {
            state.apply(LifecycleAction::WindowCloseRequested)
        }
        Message::WindowCloseRequested(_) => Task::none(),
        Message::WindowOpened(id) => {
            state.window = Some(id);
            Task::none()
        }
    }
}

fn subscription(state: &State) -> Subscription<Message> {
    let close_requests = window::close_requests().map(Message::WindowCloseRequested);
    let controller_ticks = iced::time::every(CONTROLLER_TICK).map(|_| Message::ControllerTick);
    let mut subscriptions = vec![close_requests, controller_ticks];

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    if state.tray.is_some() {
        subscriptions.push(crate::tray::subscription().map(Message::Action));
    }

    if let Some(instance) = &state.instance {
        subscriptions.push(instance.subscription().map(Message::Instance));
    }

    Subscription::batch(subscriptions)
}

fn view(state: &State, _window: window::Id) -> Element<'_, Message> {
    let status_color = match state.device_phase {
        DevicePhase::Connected => ACCENT,
        DevicePhase::Error => DANGER,
        DevicePhase::Discovering | DevicePhase::Connecting | DevicePhase::NoDevice => WARNING,
    };
    let status = container(
        row![
            text("●").size(11).color(status_color),
            text(state.phase_text()).size(14)
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding([9, 14])
    .style(move |_| badge_style(status_color));

    let english_selected = state.language == Language::English;
    let chinese_selected = state.language == Language::SimplifiedChinese;
    let header_controls = row![
        status,
        button(text(state.text("action.refresh")).size(14))
            .padding([9, 14])
            .style(|_, status| header_button_style(status, false))
            .on_press(Message::Refresh),
        button(text("EN").size(14))
            .padding([9, 14])
            .style(move |_, status| header_button_style(status, english_selected))
            .on_press(Message::SetLanguage(Language::English)),
        button(text("中文").size(14))
            .padding([9, 14])
            .style(move |_, status| header_button_style(status, chinese_selected))
            .on_press(Message::SetLanguage(Language::SimplifiedChinese)),
    ]
    .spacing(6);

    let header = row![
        column![
            text(state.text("app.title")).size(28),
            text(state.text("app.subtitle")).size(13).color(TEXT_MUTED),
        ]
        .spacing(3),
        space().width(Fill),
        header_controls,
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let mut content = column![header].spacing(18).width(Fill);
    if let Some(notice) = &state.runtime_notice {
        content = content.push(
            container(text(notice).size(13).color(Color::WHITE))
                .padding([10, 14])
                .width(Fill)
                .style(|_| notice_style()),
        );
    }

    content = content.push(match state.device_phase {
        DevicePhase::Connected => connected_view(state),
        DevicePhase::Discovering | DevicePhase::Connecting => centered_state(
            state.phase_text(),
            state.text("status.read_only_detail"),
            WARNING,
        ),
        DevicePhase::NoDevice => centered_state(
            state.text("status.no_device"),
            state.text("status.read_only_detail"),
            WARNING,
        ),
        DevicePhase::Error => centered_state(
            state.text("status.error"),
            state.text("status.read_only_detail"),
            DANGER,
        ),
    });

    content = content.push(
        text(state.text("app.disclaimer"))
            .size(11)
            .color(TEXT_MUTED),
    );

    container(content)
        .padding(24)
        .width(Fill)
        .height(Fill)
        .style(|_| container::Style::default().background(BACKGROUND))
        .into()
}

fn connected_view(state: &State) -> Element<'_, Message> {
    let device_name = state
        .device
        .as_ref()
        .map_or("Stream 4x5", |device| device.display_name.as_str());
    let firmware = state
        .device
        .as_ref()
        .and_then(|device| device.firmware_version.as_deref())
        .unwrap_or(state.text("device.unavailable"));
    let sample_rate = state.integer(&ControlId::SampleRate).map_or_else(
        || state.text("device.unavailable").to_owned(),
        |value| format!("{value} Hz"),
    );
    let clock = state.choice(&ControlId::ClockSource).map_or_else(
        || state.text("device.unavailable").to_owned(),
        |value| match value {
            "external" => state.text("device.external").to_owned(),
            _ => state.text("device.internal").to_owned(),
        },
    );
    let asio_buffer = state.integer(&ControlId::AsioBufferSize).map_or_else(
        || state.text("device.unavailable").to_owned(),
        |value| format!("{value} samples"),
    );

    let write_detail = if [ChannelId::Input1, ChannelId::Input2]
        .into_iter()
        .any(|channel| state.can_write(&ControlId::InputVolume { channel }))
    {
        state.text("status.hardware_writes_detail")
    } else {
        state.text("status.read_only_detail")
    };
    let overview = container(
        row![
            column![
                text(device_name).size(23),
                text(write_detail).size(13).color(TEXT_MUTED),
            ]
            .spacing(5)
            .width(Length::FillPortion(3)),
            info_value(state.text("device.firmware"), firmware.to_owned()),
            info_value(state.text("device.sample_rate"), sample_rate),
            info_value(state.text("device.clock"), clock),
            info_value(state.text("device.asio_buffer"), asio_buffer),
        ]
        .spacing(18)
        .align_y(Alignment::Center),
    )
    .padding(18)
    .width(Fill)
    .style(|_| card_style());

    let inputs = row![
        input_card(state, ChannelId::Input1, "channel.input_1"),
        input_card(state, ChannelId::Input2, "channel.input_2"),
        fixed_input_card(state, "channel.input_3_4"),
    ]
    .spacing(14);

    let outputs = row![
        output_card(state, BusId::Output1_2, "channel.output_1_2"),
        output_card(state, BusId::Output3_4, "channel.output_3_4"),
        output_card(state, BusId::Output5_6, "channel.output_5_6"),
    ]
    .spacing(14);

    let body = column![
        overview,
        section_heading(state.text("section.inputs")),
        inputs,
        section_heading(state.text("section.outputs")),
        outputs,
        container(
            text(state.text("status.meters_pending"))
                .size(12)
                .color(TEXT_MUTED)
        )
        .padding([8, 12])
        .style(|_| subtle_style()),
    ]
    .spacing(14)
    .width(Fill);

    scrollable(body).height(Fill).into()
}

fn input_card<'a>(
    state: &'a State,
    channel: ChannelId,
    title_key: &'static str,
) -> Element<'a, Message> {
    let gain = state.decibels(&ControlId::InputVolume { channel });
    let mute = state.boolean(&ControlId::InputMute { channel });
    let phantom_id = ControlId::PhantomPower { channel };
    let phantom = state.boolean(&phantom_id);
    let high_pass_id = ControlId::HighPass { channel };
    let high_pass = state.boolean(&high_pass_id);
    let phase_id = ControlId::PhaseInvert { channel };
    let phase = state.boolean(&phase_id);
    let high_pass_action = high_pass
        .filter(|_| state.can_write(&high_pass_id))
        .map(|enabled| Message::SetHighPass(channel, !enabled));
    let phase_action = phase
        .filter(|_| state.can_write(&phase_id))
        .map(|enabled| Message::SetPhaseInvert(channel, !enabled));
    let phantom_action = phantom
        .filter(|_| state.can_write(&phantom_id))
        .map(|enabled| Message::SetPhantomPower(channel, !enabled));
    let toggles = row![
        state_pill(state, "control.phantom", phantom, phantom_action),
        state_pill(state, "control.high_pass", high_pass, high_pass_action),
        state_pill(state, "control.phase", phase, phase_action),
    ]
    .spacing(6);
    channel_card(
        state,
        state.text(title_key),
        gain,
        mute,
        -7.0..=48.0,
        Some(toggles.into()),
        state
            .can_write(&ControlId::InputVolume { channel })
            .then_some(channel),
    )
}

fn fixed_input_card<'a>(state: &'a State, title_key: &'static str) -> Element<'a, Message> {
    container(
        column![
            row![
                column![
                    text(state.text(title_key)).size(16),
                    text(state.text("control.gain")).size(11).color(TEXT_MUTED),
                ]
                .spacing(2),
                space().width(Fill),
                text(state.text("state.fixed")).size(22).color(TEXT_MUTED),
            ]
            .align_y(Alignment::Center),
            text(state.text("status.fixed_input_detail"))
                .size(11)
                .color(TEXT_MUTED),
        ]
        .spacing(14),
    )
    .padding(16)
    .width(Length::FillPortion(1))
    .style(|_| card_style())
    .into()
}

fn output_card<'a>(state: &'a State, bus: BusId, title_key: &'static str) -> Element<'a, Message> {
    let gain = state.decibels(&ControlId::OutputVolume { bus });
    let mute_id = ControlId::OutputMute { bus };
    let mute_status: Option<Element<'a, Message>> = state.supports_control(&mute_id).then(|| {
        row![state_pill(
            state,
            "control.output_mute",
            state.boolean(&mute_id),
            None
        )]
        .spacing(6)
        .into()
    });
    channel_card(
        state,
        state.text(title_key),
        gain,
        None,
        -60.0..=0.0,
        mute_status,
        None,
    )
}

fn channel_card<'a>(
    state: &'a State,
    title: &'a str,
    gain: Option<f32>,
    endpoint_muted: Option<bool>,
    range: std::ops::RangeInclusive<f32>,
    toggles: Option<Element<'a, Message>>,
    editable_input: Option<ChannelId>,
) -> Element<'a, Message> {
    let value = if endpoint_muted == Some(true) {
        state.text("state.muted").to_owned()
    } else {
        gain.map_or_else(
            || state.text("device.unavailable").to_owned(),
            |gain| format!("{gain:.1} dB"),
        )
    };
    let bar_value = if endpoint_muted == Some(true) {
        *range.start()
    } else {
        gain.unwrap_or(*range.start())
    };
    let gain_control: Element<'a, Message> = if let Some(channel) = editable_input {
        slider(range.clone(), bar_value, move |value| {
            Message::SetInputGain(channel, value)
        })
        .step(1.0_f32)
        .into()
    } else {
        progress_bar(range, bar_value)
            .girth(7)
            .style(|_| gain_bar_style())
            .into()
    };
    let mut content = column![
        row![
            column![
                text(title).size(16),
                text(state.text("control.gain")).size(11).color(TEXT_MUTED),
            ]
            .spacing(2),
            space().width(Fill),
            text(value).size(22).color(ACCENT),
        ]
        .align_y(Alignment::Center),
        gain_control,
    ]
    .spacing(14);
    if let Some(toggles) = toggles {
        content = content.push(toggles);
    }
    container(content)
        .padding(16)
        .width(Length::FillPortion(1))
        .style(|_| card_style())
        .into()
}

fn state_pill<'a>(
    state: &'a State,
    label_key: &'static str,
    active: Option<bool>,
    action: Option<Message>,
) -> Element<'a, Message> {
    let label = match active {
        Some(true) => format!("{} · {}", state.text(label_key), state.text("state.on")),
        Some(false) => format!("{} · {}", state.text(label_key), state.text("state.off")),
        None => format!(
            "{} · {}",
            state.text(label_key),
            state.text("device.unavailable")
        ),
    };
    let is_active = active == Some(true);
    let label = text(label)
        .size(10)
        .color(if is_active { Color::WHITE } else { TEXT_MUTED });
    if let Some(action) = action {
        button(label)
            .padding([5, 8])
            .style(move |_, status| pill_button_style(status, is_active))
            .on_press(action)
            .into()
    } else {
        container(label)
            .padding([5, 8])
            .style(move |_| pill_style(is_active))
            .into()
    }
}

fn info_value(label: &str, value: String) -> Element<'_, Message> {
    column![text(label).size(10).color(TEXT_MUTED), text(value).size(15),]
        .spacing(3)
        .into()
}

fn section_heading(label: &str) -> Element<'_, Message> {
    row![text(label).size(17), space().width(Fill)]
        .align_y(Alignment::Center)
        .into()
}

fn centered_state<'a>(title: &'a str, detail: &'a str, color: Color) -> Element<'a, Message> {
    container(
        column![
            text("●").size(30).color(color),
            text(title).size(24),
            text(detail).size(13).color(TEXT_MUTED),
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .padding(32)
    .center(Fill)
    .style(|_| card_style())
    .into()
}

fn application_theme() -> Theme {
    Theme::custom(
        "Lewitt graphite",
        Palette {
            background: BACKGROUND,
            text: Color::from_rgb(0.92, 0.94, 0.97),
            primary: ACCENT,
            success: ACCENT,
            warning: WARNING,
            danger: DANGER,
        },
    )
}

fn card_style() -> container::Style {
    container::Style::default()
        .background(SURFACE)
        .border(border::rounded(14).width(1).color(BORDER))
}

fn subtle_style() -> container::Style {
    container::Style::default()
        .background(SURFACE_RAISED)
        .border(border::rounded(8).width(1).color(BORDER))
}

fn badge_style(color: Color) -> container::Style {
    container::Style::default()
        .background(SURFACE_RAISED)
        .border(border::rounded(20).width(1).color(color))
}

fn header_button_style(status: button::Status, selected: bool) -> button::Style {
    let border_color = if selected { ACCENT } else { BORDER };
    let background = match status {
        button::Status::Hovered => Color::from_rgb(0.11, 0.13, 0.16),
        button::Status::Pressed => SURFACE,
        button::Status::Active | button::Status::Disabled => SURFACE_RAISED,
    };

    button::Style {
        background: Some(Background::Color(background)),
        text_color: if status == button::Status::Disabled {
            TEXT_MUTED
        } else {
            Color::WHITE
        },
        border: border::rounded(20).width(1).color(
            if matches!(status, button::Status::Hovered | button::Status::Pressed) {
                ACCENT
            } else {
                border_color
            },
        ),
        ..button::Style::default()
    }
}

fn notice_style() -> container::Style {
    container::Style::default()
        .background(Color::from_rgb(0.24, 0.10, 0.11))
        .border(border::rounded(8).width(1).color(DANGER))
}

fn pill_style(active: bool) -> container::Style {
    let color = if active { ACCENT } else { BORDER };
    container::Style::default()
        .background(if active {
            Color::from_rgb(0.08, 0.29, 0.26)
        } else {
            SURFACE_RAISED
        })
        .border(border::rounded(16).width(1).color(color))
}

fn pill_button_style(status: button::Status, active: bool) -> button::Style {
    let color = if active { ACCENT } else { BORDER };
    let background = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        Color::from_rgb(0.11, 0.20, 0.20)
    } else if active {
        Color::from_rgb(0.08, 0.29, 0.26)
    } else {
        SURFACE_RAISED
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: if active { Color::WHITE } else { TEXT_MUTED },
        border: border::rounded(16).width(1).color(color),
        ..button::Style::default()
    }
}

fn gain_bar_style() -> progress_bar::Style {
    progress_bar::Style {
        background: Background::Color(SURFACE_RAISED),
        bar: Background::Color(ACCENT),
        border: border::rounded(6),
    }
}

fn backend_notice_key(error: &BackendError) -> &'static str {
    match error {
        BackendError::DriverMissing { .. } => "notice.driver_missing",
        BackendError::PermissionDenied { .. } => "notice.permission_denied",
        BackendError::Busy { .. } => "notice.device_busy",
        BackendError::Disconnected => "notice.device_disconnected",
        BackendError::UnsupportedFirmware { .. } | BackendError::Unsupported { .. } => {
            "notice.operation_unavailable"
        }
        BackendError::InvalidValue { .. } => "notice.invalid_value",
        _ => "notice.device_read_failed",
    }
}

fn window_settings() -> window::Settings {
    window::Settings {
        size: WINDOW_SIZE,
        min_size: Some(MIN_WINDOW_SIZE),
        exit_on_close_request: false,
        ..window::Settings::default()
    }
}

fn restore_and_raise_window(id: window::Id) -> Task<Message> {
    window::minimize(id, false)
        .chain(window::set_level(id, window::Level::AlwaysOnTop))
        .chain(window::gain_focus(id))
        .chain(window::set_level(id, window::Level::Normal))
}

#[cfg(target_os = "windows")]
fn spawn_platform_controller() -> Result<Controller, lewitt_core::ControllerError> {
    Controller::spawn(
        lewitt_backend_windows::WindowsBackend::new(),
        ControllerConfig::default(),
    )
}

#[cfg(target_os = "linux")]
fn spawn_platform_controller() -> Result<Controller, lewitt_core::ControllerError> {
    Controller::spawn(
        lewitt_backend_linux::LinuxBackend::new(),
        ControllerConfig::default(),
    )
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn spawn_platform_controller() -> Result<Controller, lewitt_core::ControllerError> {
    Err(lewitt_core::ControllerError::Spawn(
        "no platform backend is available".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_hardware_polling_targets_sixty_hertz() {
        assert_eq!(CONTROLLER_TICK, Duration::from_micros(16_667));
        assert_eq!(VISIBLE_SNAPSHOT_INTERVAL, Duration::from_micros(16_667));
        assert_eq!(HIDDEN_SNAPSHOT_INTERVAL, Duration::from_secs(5));
    }

    #[test]
    fn foreground_boot_reserves_a_window() {
        let mut state = State::without_controller(false, None);
        let _task = state.show_window();
        assert!(state.window.is_some());
        assert!(state.lifecycle.window_visible());
    }

    #[test]
    fn background_boot_starts_without_a_window() {
        let state = State::without_controller(true, None);
        assert!(state.window.is_none());
        assert!(!state.lifecycle.window_visible());
    }

    #[test]
    fn window_close_is_managed_by_the_daemon() {
        let settings = window_settings();
        assert!(!settings.exit_on_close_request);
        assert_eq!(settings.size, WINDOW_SIZE);
        assert_eq!(settings.min_size, Some(MIN_WINDOW_SIZE));
        assert_eq!(settings.level, window::Level::Normal);
    }

    #[test]
    fn controller_snapshot_values_are_renderable() {
        let mut state = State::without_controller(false, None);
        state.snapshot.controls.insert(
            ControlId::InputVolume {
                channel: ChannelId::Input1,
            },
            ControlValue::Decibels(12.5),
        );
        state.snapshot.controls.insert(
            ControlId::PhantomPower {
                channel: ChannelId::Input1,
            },
            ControlValue::Boolean(true),
        );
        assert_eq!(
            state.decibels(&ControlId::InputVolume {
                channel: ChannelId::Input1,
            }),
            Some(12.5)
        );
        assert_eq!(
            state.boolean(&ControlId::PhantomPower {
                channel: ChannelId::Input1,
            }),
            Some(true)
        );
    }

    #[test]
    fn optional_output_mute_is_gated_by_backend_capabilities() {
        let mut state = State::without_controller(false, None);
        let mute = ControlId::OutputMute {
            bus: BusId::Output1_2,
        };
        state
            .snapshot
            .controls
            .insert(mute.clone(), ControlValue::Boolean(false));
        assert!(!state.supports_control(&mute));

        state.capabilities = Some(DeviceCapabilities {
            model: "Stream 4x5".into(),
            firmware: lewitt_core::FirmwareStatus::UnknownReadOnly { version: None },
            controls: vec![lewitt_core::ControlDescriptor {
                id: mute.clone(),
                kind: lewitt_core::ControlKind::Discrete,
                value_kind: lewitt_core::ValueKind::Boolean,
                access: lewitt_core::ControlAccess::ReadOnly,
                range: None,
                choices: Vec::new(),
            }],
            meter_sources: Vec::new(),
        });
        assert!(state.supports_control(&mute));
    }

    #[test]
    fn backend_details_are_not_rendered_in_the_gui_notice() {
        let error = BackendError::ProtocolMismatch {
            details: "private low-level ABI detail".into(),
        };
        let catalog = Catalog::for_language(Language::English);
        let notice = catalog
            .get(backend_notice_key(&error))
            .expect("every backend notice must be localized");

        assert_eq!(
            notice,
            "Device data could not be read. See the local log for details."
        );
        assert!(!notice.contains("private low-level ABI detail"));
    }

    #[test]
    fn background_poll_errors_are_visible_without_exposing_details() {
        let mut state = State::without_controller(false, None);
        state.report_backend_error(
            WorkerOperation::ReadMeters,
            &BackendError::ProtocolMismatch {
                details: "private meter transport detail".into(),
            },
        );
        assert_eq!(
            state.runtime_notice.as_deref(),
            Some("Device data could not be read. See the local log for details.")
        );
        assert!(
            !state
                .runtime_notice
                .as_deref()
                .is_some_and(|notice| notice.contains("private meter transport detail"))
        );
    }
}
