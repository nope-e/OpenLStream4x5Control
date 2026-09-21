//! Read-only Windows backend for the registered Stream 4x5 vendor API.
//!
//! The backend resolves a narrowly typed read subset from the registered
//! absolute DLL path. It never resolves write, raw vendor, firmware, or DFU
//! exports. All FFI stays in this crate and all hardware access is serialized
//! by the shared controller worker.

use lewitt_core::{
    BackendError, BackendResult, ControlCommand, ControlId, ControlValue, DeviceBackend,
    DeviceCapabilities, DeviceEvent, DeviceId, DeviceInfo, DeviceSnapshot, MeterFrame,
};
#[cfg(windows)]
use lewitt_core::{
    BusId, ChannelId, ControlAccess, ControlDescriptor, ControlKind, FirmwareStatus, NumericRange,
    STREAM_4X5_PRODUCT_ID, STREAM_4X5_VENDOR_ID, ValueKind,
};
#[cfg(windows)]
use std::collections::BTreeMap;
use std::time::Duration;

#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod vendor_api;

#[cfg(windows)]
pub use registry::locate_registered_vendor_api;

#[cfg(not(windows))]
use std::path::PathBuf;
#[cfg(windows)]
use vendor_api::VendorApi;

pub const VENDOR_API_CLSID: &str = "{ADACFE1D-A8E1-4606-9093-3A7418223B78}";

#[cfg(not(windows))]
pub fn locate_registered_vendor_api() -> BackendResult<PathBuf> {
    Err(BackendError::DriverMissing {
        details: "the vendor API registry is only available on Windows".into(),
    })
}

#[cfg(windows)]
struct OpenDevice {
    handle: u32,
    info: DeviceInfo,
}

#[cfg(windows)]
pub struct WindowsBackend {
    api: Option<VendorApi>,
    discovered: BTreeMap<DeviceId, u32>,
    open: Option<OpenDevice>,
    snapshot_revision: u64,
    meter_sequence: u64,
}

#[cfg(not(windows))]
#[derive(Debug)]
pub struct WindowsBackend;

impl WindowsBackend {
    #[must_use]
    pub const fn new() -> Self {
        #[cfg(windows)]
        {
            Self {
                api: None,
                discovered: BTreeMap::new(),
                open: None,
                snapshot_revision: 0,
                meter_sequence: 0,
            }
        }

        #[cfg(not(windows))]
        {
            Self
        }
    }

    /// Resolve and validate the registered 64-bit vendor API path without
    /// loading the DLL.
    pub fn diagnose_vendor_api(&self) -> BackendResult<std::path::PathBuf> {
        locate_registered_vendor_api()
    }
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
impl WindowsBackend {
    fn ensure_api(&mut self) -> BackendResult<&VendorApi> {
        if self.api.is_none() {
            let path = self.diagnose_vendor_api()?;
            self.api = Some(VendorApi::load(&path)?);
        }
        self.api
            .as_ref()
            .ok_or_else(|| BackendError::DriverMissing {
                details: "vendor API failed to initialize".into(),
            })
    }

    fn require_open(&self) -> BackendResult<&OpenDevice> {
        self.open.as_ref().ok_or(BackendError::Disconnected)
    }

    fn inspect_opened_device(
        api: &VendorApi,
        handle: u32,
        index: u32,
    ) -> BackendResult<Option<DeviceInfo>> {
        let instance_id = api.device_instance_id(handle)?;
        let normalized = instance_id.to_ascii_uppercase();
        if !normalized.contains("VID_29C2&PID_0011") {
            return Ok(None);
        }

        let properties = api.device_properties(handle)?;
        let firmware = read_u32(&properties, 8)?;
        let description = read_utf16(&properties, 524)?;
        if !description.to_ascii_lowercase().contains("stream 4x5") {
            return Err(BackendError::ProtocolMismatch {
                details: "VID/PID matched Stream 4x5 but the vendor model description did not"
                    .into(),
            });
        }

        Ok(Some(DeviceInfo {
            id: DeviceId::new(format!("windows-vendor-api:{index}")),
            display_name: description,
            vendor_id: STREAM_4X5_VENDOR_ID,
            product_id: STREAM_4X5_PRODUCT_ID,
            firmware_version: Some(format!("0x{firmware:04X}")),
        }))
    }

    fn close_current(&mut self) -> BackendResult<()> {
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        let Some(api) = self.api.as_ref() else {
            return Err(BackendError::ProtocolMismatch {
                details: "an open handle existed without a loaded vendor API".into(),
            });
        };
        api.close(open.handle)
    }

    fn readonly_capabilities(&self) -> BackendResult<DeviceCapabilities> {
        let open = self.require_open()?;
        let controls = vec![
            decibel_control(
                ControlId::InputVolume {
                    channel: ChannelId::Input1,
                },
                Some((-7.0, 48.0)),
            ),
            decibel_control(
                ControlId::InputVolume {
                    channel: ChannelId::Input2,
                },
                Some((-7.0, 48.0)),
            ),
            boolean_control(ControlId::InputMute {
                channel: ChannelId::Input1,
            }),
            boolean_control(ControlId::InputMute {
                channel: ChannelId::Input2,
            }),
            boolean_control(ControlId::PhantomPower {
                channel: ChannelId::Input1,
            }),
            boolean_control(ControlId::PhantomPower {
                channel: ChannelId::Input2,
            }),
            boolean_control(ControlId::HighPass {
                channel: ChannelId::Input1,
            }),
            boolean_control(ControlId::HighPass {
                channel: ChannelId::Input2,
            }),
            boolean_control(ControlId::PhaseInvert {
                channel: ChannelId::Input1,
            }),
            boolean_control(ControlId::PhaseInvert {
                channel: ChannelId::Input2,
            }),
            decibel_control(
                ControlId::OutputVolume {
                    bus: BusId::Output1_2,
                },
                Some((-60.0, 0.0)),
            ),
            decibel_control(
                ControlId::OutputVolume {
                    bus: BusId::Output3_4,
                },
                Some((-60.0, 0.0)),
            ),
            decibel_control(
                ControlId::OutputVolume {
                    bus: BusId::Output5_6,
                },
                Some((-60.0, 0.0)),
            ),
            boolean_control(ControlId::OutputMute {
                bus: BusId::Output1_2,
            }),
            boolean_control(ControlId::OutputMute {
                bus: BusId::Output3_4,
            }),
            boolean_control(ControlId::OutputMute {
                bus: BusId::Output5_6,
            }),
            integer_control(ControlId::SampleRate),
            choice_control(ControlId::ClockSource, &["internal", "external"]),
            integer_control(ControlId::AsioBufferSize),
        ];
        Ok(DeviceCapabilities {
            model: open.info.display_name.clone(),
            firmware: FirmwareStatus::UnknownReadOnly {
                version: open.info.firmware_version.clone(),
            },
            controls,
            meter_sources: Vec::new(),
        })
    }

    fn decode_snapshot(
        &mut self,
        settings: &[u8; 40],
        output_mutes: [bool; 3],
        sample_rate: u32,
        clock_source: &[u8; 340],
        asio_info: Option<&[u8; 196]>,
    ) -> BackendResult<DeviceSnapshot> {
        let mut controls = decode_hardware_settings(settings)?;
        for (bus, muted) in [BusId::Output1_2, BusId::Output3_4, BusId::Output5_6]
            .into_iter()
            .zip(output_mutes)
        {
            insert_bool(&mut controls, ControlId::OutputMute { bus }, muted);
        }
        let sample_rate =
            i32::try_from(sample_rate).map_err(|_| BackendError::ProtocolMismatch {
                details: "sample rate does not fit the shared integer model".into(),
            })?;
        controls.insert(ControlId::SampleRate, ControlValue::Integer(sample_rate));
        let clock_type = read_u32(clock_source, 276)?;
        controls.insert(
            ControlId::ClockSource,
            ControlValue::Choice(if clock_type == 1 {
                "external".into()
            } else {
                "internal".into()
            }),
        );
        if let Some(asio_info) = asio_info {
            let buffer_size = read_u32(asio_info, 28)?;
            if let Ok(buffer_size) = i32::try_from(buffer_size) {
                controls.insert(
                    ControlId::AsioBufferSize,
                    ControlValue::Integer(buffer_size),
                );
            }
        }

        self.snapshot_revision = self.snapshot_revision.saturating_add(1);
        Ok(DeviceSnapshot {
            revision: self.snapshot_revision,
            controls,
        })
    }
}

#[cfg(windows)]
impl DeviceBackend for WindowsBackend {
    fn enumerate(&mut self) -> BackendResult<Vec<DeviceInfo>> {
        self.discovered.clear();
        let api = self.ensure_api()?;
        let count = match api.enumerate() {
            Ok(count) => count,
            Err(BackendError::Disconnected) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut devices = Vec::new();
        let mut discovered = BTreeMap::new();
        for index in 0..count {
            let handle = api.open(index)?;
            let inspected = Self::inspect_opened_device(api, handle, index);
            let closed = api.close(handle);
            let candidate = inspected?;
            closed?;
            if let Some(info) = candidate {
                discovered.insert(info.id.clone(), index);
                devices.push(info);
            }
        }
        self.discovered = discovered;
        Ok(devices)
    }

    fn open(&mut self, device: &DeviceId) -> BackendResult<()> {
        self.close_current()?;
        let index = self
            .discovered
            .get(device)
            .copied()
            .ok_or(BackendError::Disconnected)?;
        let api = self.ensure_api()?;
        let handle = api.open(index)?;
        match Self::inspect_opened_device(api, handle, index) {
            Ok(Some(info)) if &info.id == device => {
                self.open = Some(OpenDevice { handle, info });
                Ok(())
            }
            Ok(_) => {
                let _ = api.close(handle);
                Err(BackendError::Disconnected)
            }
            Err(error) => {
                let _ = api.close(handle);
                Err(error)
            }
        }
    }

    fn close(&mut self) -> BackendResult<()> {
        self.close_current()
    }

    fn capabilities(&mut self) -> BackendResult<DeviceCapabilities> {
        self.readonly_capabilities()
    }

    fn read_snapshot(&mut self) -> BackendResult<DeviceSnapshot> {
        let handle = self.require_open()?.handle;
        let api = self.ensure_api()?;
        let settings = api.stream4x5_settings(handle)?;
        let output_mutes = read_output_mutes(api, handle)?;
        let sample_rate = api.current_sample_rate(handle)?;
        let clock_source = api.current_clock_source(handle)?;
        let asio_info = api.asio_instance_info().ok();
        self.decode_snapshot(
            &settings,
            output_mutes,
            sample_rate,
            &clock_source,
            asio_info.as_ref(),
        )
    }

    fn set_control(&mut self, command: &ControlCommand) -> BackendResult<()> {
        Err(BackendError::Unsupported {
            feature: format!(
                "write to {:?}; Windows backend is read-only",
                command.control
            ),
        })
    }

    fn read_back(&mut self, control: &ControlId) -> BackendResult<ControlValue> {
        self.read_snapshot()?
            .controls
            .get(control)
            .cloned()
            .ok_or_else(|| BackendError::Unsupported {
                feature: format!("read of {control:?}"),
            })
    }

    fn read_meters(&mut self) -> BackendResult<MeterFrame> {
        self.require_open()?;
        self.meter_sequence = self.meter_sequence.saturating_add(1);
        Ok(MeterFrame {
            sequence: self.meter_sequence,
            samples: Vec::new(),
        })
    }

    fn poll_events(&mut self, _timeout: Duration) -> BackendResult<Vec<DeviceEvent>> {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
impl Drop for WindowsBackend {
    fn drop(&mut self) {
        let _ = self.close_current();
    }
}

#[cfg(not(windows))]
impl DeviceBackend for WindowsBackend {
    fn enumerate(&mut self) -> BackendResult<Vec<DeviceInfo>> {
        Err(BackendError::DriverMissing {
            details: "Windows vendor API is only available on Windows".into(),
        })
    }

    fn open(&mut self, _device: &DeviceId) -> BackendResult<()> {
        Err(BackendError::Unsupported {
            feature: "Windows vendor API on this platform".into(),
        })
    }

    fn close(&mut self) -> BackendResult<()> {
        Ok(())
    }

    fn capabilities(&mut self) -> BackendResult<DeviceCapabilities> {
        Err(BackendError::Disconnected)
    }

    fn read_snapshot(&mut self) -> BackendResult<DeviceSnapshot> {
        Err(BackendError::Disconnected)
    }

    fn set_control(&mut self, _command: &ControlCommand) -> BackendResult<()> {
        Err(BackendError::Unsupported {
            feature: "Windows backend writes".into(),
        })
    }

    fn read_back(&mut self, _control: &ControlId) -> BackendResult<ControlValue> {
        Err(BackendError::Disconnected)
    }

    fn read_meters(&mut self) -> BackendResult<MeterFrame> {
        Err(BackendError::Disconnected)
    }

    fn poll_events(&mut self, _timeout: Duration) -> BackendResult<Vec<DeviceEvent>> {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
fn read_u32(bytes: &[u8], offset: usize) -> BackendResult<u32> {
    let raw = bytes
        .get(offset..offset.saturating_add(4))
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| BackendError::ProtocolMismatch {
            details: format!("u32 field at offset {offset} is outside its structure"),
        })?;
    Ok(u32::from_le_bytes(raw))
}

#[cfg(any(windows, test))]
fn read_i16(bytes: &[u8], offset: usize) -> BackendResult<i16> {
    let raw = bytes
        .get(offset..offset.saturating_add(2))
        .and_then(|value| value.try_into().ok())
        .ok_or_else(|| BackendError::ProtocolMismatch {
            details: format!("i16 field at offset {offset} is outside its structure"),
        })?;
    Ok(i16::from_le_bytes(raw))
}

#[cfg(any(windows, test))]
fn read_q8_8(bytes: &[u8], offset: usize) -> BackendResult<f32> {
    Ok(f32::from(read_i16(bytes, offset)?) / 256.0)
}

#[cfg(any(windows, test))]
fn average_q8_8(bytes: &[u8], first: usize, second: usize) -> BackendResult<f32> {
    Ok(f32::midpoint(
        read_q8_8(bytes, first)?,
        read_q8_8(bytes, second)?,
    ))
}

#[cfg(windows)]
fn read_utf16(bytes: &[u8], offset: usize) -> BackendResult<String> {
    let tail = bytes
        .get(offset..)
        .ok_or_else(|| BackendError::ProtocolMismatch {
            details: format!("UTF-16 field at offset {offset} is outside its structure"),
        })?;
    let (chunks, _) = tail.as_chunks::<2>();
    let units = chunks
        .iter()
        .map(|chunk| u16::from_le_bytes(*chunk))
        .take_while(|unit| *unit != 0)
        .collect::<Vec<_>>();
    String::from_utf16(&units).map_err(|error| BackendError::ProtocolMismatch {
        details: format!("device description is not valid UTF-16: {error}"),
    })
}

#[cfg(windows)]
fn decibel_control(id: ControlId, range: Option<(f32, f32)>) -> ControlDescriptor {
    ControlDescriptor {
        id,
        kind: ControlKind::Continuous,
        value_kind: ValueKind::Decibels,
        access: ControlAccess::ReadOnly,
        range: range.map(|(minimum, maximum)| NumericRange {
            minimum,
            maximum,
            step: None,
        }),
        choices: Vec::new(),
    }
}

#[cfg(windows)]
fn boolean_control(id: ControlId) -> ControlDescriptor {
    ControlDescriptor {
        id,
        kind: ControlKind::Discrete,
        value_kind: ValueKind::Boolean,
        access: ControlAccess::ReadOnly,
        range: None,
        choices: Vec::new(),
    }
}

#[cfg(windows)]
fn integer_control(id: ControlId) -> ControlDescriptor {
    ControlDescriptor {
        id,
        kind: ControlKind::Discrete,
        value_kind: ValueKind::Integer,
        access: ControlAccess::ReadOnly,
        range: None,
        choices: Vec::new(),
    }
}

#[cfg(windows)]
fn choice_control(id: ControlId, choices: &[&str]) -> ControlDescriptor {
    ControlDescriptor {
        id,
        kind: ControlKind::Discrete,
        value_kind: ValueKind::Choice,
        access: ControlAccess::ReadOnly,
        range: None,
        choices: choices.iter().map(ToString::to_string).collect(),
    }
}

#[cfg(windows)]
fn insert_db(controls: &mut BTreeMap<ControlId, ControlValue>, id: ControlId, value: f32) {
    controls.insert(id, ControlValue::Decibels(value));
}

#[cfg(windows)]
fn insert_bool(controls: &mut BTreeMap<ControlId, ControlValue>, id: ControlId, value: bool) {
    controls.insert(id, ControlValue::Boolean(value));
}

#[cfg(windows)]
fn decode_hardware_settings(
    settings: &[u8; 40],
) -> BackendResult<BTreeMap<ControlId, ControlValue>> {
    let mut controls = BTreeMap::new();
    decode_output_settings(settings, &mut controls)?;
    decode_input_settings(settings, &mut controls)?;
    Ok(controls)
}

#[cfg(windows)]
fn decode_output_settings(
    settings: &[u8; 40],
    controls: &mut BTreeMap<ControlId, ControlValue>,
) -> BackendResult<()> {
    for (bus, first_gain, second_gain) in [
        (BusId::Output1_2, 0, 2),
        (BusId::Output3_4, 4, 6),
        (BusId::Output5_6, 8, 10),
    ] {
        insert_db(
            controls,
            ControlId::OutputVolume { bus },
            average_q8_8(settings, first_gain, second_gain)?,
        );
    }
    Ok(())
}

#[cfg(windows)]
fn read_output_mutes(api: &VendorApi, handle: u32) -> BackendResult<[bool; 3]> {
    let mut attenuation = [0_i32; 6];
    for (index, value) in attenuation.iter_mut().enumerate() {
        *value = api.output_attenuation_raw(
            handle,
            1,
            u8::try_from(index).map_err(|_| BackendError::ProtocolMismatch {
                details: "device output index does not fit the DSP property ABI".into(),
            })?,
        )?;
    }
    Ok(decode_output_mutes(attenuation))
}

#[cfg(any(windows, test))]
fn decode_output_mutes(attenuation: [i32; 6]) -> [bool; 3] {
    [
        attenuation[0] == 0 && attenuation[1] == 0,
        attenuation[2] == 0 && attenuation[3] == 0,
        attenuation[4] == 0 && attenuation[5] == 0,
    ]
}

#[cfg(windows)]
fn decode_input_settings(
    settings: &[u8; 40],
    controls: &mut BTreeMap<ControlId, ControlValue>,
) -> BackendResult<()> {
    for (channel, gain, mute) in [
        (
            ChannelId::Input1,
            read_q8_8(settings, 18)?,
            settings[26] != 0,
        ),
        (
            ChannelId::Input2,
            read_q8_8(settings, 20)?,
            settings[27] != 0,
        ),
    ] {
        insert_db(controls, ControlId::InputVolume { channel }, gain);
        insert_bool(controls, ControlId::InputMute { channel }, mute);
    }
    for (channel, high_pass, phase, phantom) in [
        (ChannelId::Input1, settings[30], settings[34], settings[38]),
        (ChannelId::Input2, settings[31], settings[35], settings[39]),
    ] {
        insert_bool(controls, ControlId::HighPass { channel }, high_pass != 0);
        insert_bool(controls, ControlId::PhaseInvert { channel }, phase != 0);
        insert_bool(controls, ControlId::PhantomPower { channel }, phantom != 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitized_fixture_decodes_to_expected_hardware_state() {
        let fixture = include_str!("../../../fixtures/stream4x5/vendor-api-read-v1.json");
        let response = fixture
            .split("\"response_hex\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("fixture contains settings response");
        let bytes = decode_hex(response);
        assert_eq!(bytes.len(), 40);
        assert_eq!(read_q8_8(&bytes, 18), Ok(44.0));
        assert_eq!(read_q8_8(&bytes, 20), Ok(-6.0));
        assert_eq!(average_q8_8(&bytes, 0, 2), Ok(-12.0));
        assert_ne!(bytes[30], 0);
        assert_ne!(bytes[38], 0);
        assert_eq!(bytes[39], 0);

        let settings: [u8; 40] = bytes
            .try_into()
            .expect("fixture is exactly one settings block");
        let controls = decode_hardware_settings(&settings).expect("fixture settings decode");
        assert!(!controls.contains_key(&ControlId::InputVolume {
            channel: ChannelId::Input3_4,
        }));
        assert!(!controls.contains_key(&ControlId::InputMute {
            channel: ChannelId::Input3_4,
        }));
        assert!(!controls.contains_key(&ControlId::OutputMute {
            bus: BusId::Output1_2,
        }));

        let attenuation = fixture
            .split("\"device_output_attenuation_raw_q8_24\": [")
            .nth(1)
            .and_then(|tail| tail.split(']').next())
            .expect("fixture contains device-output attenuation values")
            .split(',')
            .map(|value| value.trim().parse::<i32>().expect("attenuation is an i32"))
            .collect::<Vec<_>>();
        let attenuation: [i32; 6] = attenuation
            .try_into()
            .expect("fixture contains six device-output attenuation values");
        assert_eq!(decode_output_mutes(attenuation), [true, false, true]);
    }

    fn decode_hex(value: &str) -> Vec<u8> {
        value
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("hex is ASCII");
                u8::from_str_radix(text, 16).expect("fixture contains valid hex")
            })
            .collect()
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an installed vendor driver and connected Stream 4x5"]
    fn live_read_only_stream_4x5_snapshot() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("read-only enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend
            .open(&device.id)
            .expect("read-only device open succeeds");
        let snapshot = backend
            .read_snapshot()
            .expect("read-only snapshot succeeds");
        assert!(snapshot.controls.contains_key(&ControlId::SampleRate));
        assert!(snapshot.controls.contains_key(&ControlId::OutputMute {
            bus: BusId::Output1_2,
        }));
        backend.close().expect("device close succeeds");
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_backend_never_attempts_vendor_access() {
        let mut backend = WindowsBackend::new();
        assert!(matches!(
            backend.enumerate(),
            Err(BackendError::DriverMissing { .. })
        ));
    }
}
