//! Windows backend for the registered Stream 4x5 vendor API.
//!
//! The backend resolves a narrowly typed subset from the registered absolute
//! DLL path. Validated Input 1/2 controls, the three physical output gains, and
//! paired hardware output mute are written with a full-block read-modify-write;
//! raw vendor, firmware, and DFU exports remain unavailable. All FFI stays in
//! this crate and all hardware access is serialized by the shared controller
//! worker.

use lewitt_core::{
    BackendError, BackendResult, ControlCommand, ControlId, ControlValue, DeviceBackend,
    DeviceCapabilities, DeviceEvent, DeviceId, DeviceInfo, DeviceSnapshot, MeterFrame,
};
#[cfg(any(windows, test))]
use lewitt_core::{BusId, ChannelId};
#[cfg(windows)]
use lewitt_core::{CapabilityOs, STREAM_4X5_PRODUCT_ID, STREAM_4X5_VENDOR_ID, capabilities_for};
#[cfg(any(windows, test))]
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
    capabilities: DeviceCapabilities,
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

    fn capabilities_for_open_device(&self) -> BackendResult<DeviceCapabilities> {
        Ok(self.require_open()?.capabilities.clone())
    }

    fn decode_snapshot(
        &mut self,
        settings: &[u8; 40],
        sample_rate: u32,
        clock_source: &[u8; 340],
        asio_info: Option<&[u8; 196]>,
    ) -> BackendResult<DeviceSnapshot> {
        let mut controls = decode_hardware_settings(settings)?;
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
                let capabilities = match capabilities_for(
                    CapabilityOs::Windows,
                    info.firmware_version.as_deref(),
                    info.display_name.clone(),
                ) {
                    Ok(capabilities) => capabilities,
                    Err(error) => {
                        let _ = api.close(handle);
                        return Err(error);
                    }
                };
                self.open = Some(OpenDevice {
                    handle,
                    info,
                    capabilities,
                });
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
        self.capabilities_for_open_device()
    }

    fn read_snapshot(&mut self) -> BackendResult<DeviceSnapshot> {
        let handle = self.require_open()?.handle;
        let api = self.ensure_api()?;
        let settings = api.stream4x5_settings(handle)?;
        let sample_rate = api.current_sample_rate(handle)?;
        let clock_source = api.current_clock_source(handle)?;
        let asio_info = api.asio_instance_info().ok();
        self.decode_snapshot(&settings, sample_rate, &clock_source, asio_info.as_ref())
    }

    fn set_control(&mut self, command: &ControlCommand) -> BackendResult<()> {
        let open = self.require_open()?;
        let handle = open.handle;
        let descriptor = open
            .capabilities
            .descriptor(&command.control)
            .filter(|descriptor| descriptor.writable)
            .ok_or_else(|| BackendError::Unsupported {
                feature: format!("Windows write to {:?}", command.control),
            })?;
        descriptor
            .validate(&command.value)
            .map_err(|reason| BackendError::InvalidValue {
                control: command.control.clone(),
                reason,
            })?;
        let mut settings = self.ensure_api()?.stream4x5_settings(handle)?;
        apply_settings_write(&mut settings, command)?;
        self.ensure_api()?
            .set_stream4x5_settings(handle, &mut settings)
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

#[cfg(any(windows, test))]
fn apply_settings_write(settings: &mut [u8; 40], command: &ControlCommand) -> BackendResult<()> {
    match &command.control {
        ControlId::OutputVolume {
            bus: BusId::Output1_2,
        } => apply_output_gain_write(settings, command, [0, 2], [12, 13]),
        ControlId::OutputVolume {
            bus: BusId::Output3_4,
        } => apply_output_gain_write(settings, command, [4, 6], [14, 15]),
        ControlId::OutputVolume {
            bus: BusId::Output5_6,
        } => apply_output_gain_write(settings, command, [8, 10], [16, 17]),
        ControlId::InputVolume {
            channel: ChannelId::Input1,
        } => apply_input_gain_write(settings, command, 18, 26),
        ControlId::InputVolume {
            channel: ChannelId::Input2,
        } => apply_input_gain_write(settings, command, 20, 27),
        ControlId::HighPass {
            channel: ChannelId::Input1,
        } => apply_boolean_settings_write(settings, command, 30, "80 Hz high-pass"),
        ControlId::HighPass {
            channel: ChannelId::Input2,
        } => apply_boolean_settings_write(settings, command, 31, "80 Hz high-pass"),
        ControlId::PhaseInvert {
            channel: ChannelId::Input1,
        } => apply_boolean_settings_write(settings, command, 34, "phase invert"),
        ControlId::PhaseInvert {
            channel: ChannelId::Input2,
        } => apply_boolean_settings_write(settings, command, 35, "phase invert"),
        ControlId::PhantomPower {
            channel: ChannelId::Input1,
        } => apply_boolean_settings_write(settings, command, 38, "48V phantom power"),
        ControlId::PhantomPower {
            channel: ChannelId::Input2,
        } => apply_boolean_settings_write(settings, command, 39, "48V phantom power"),
        _ => Err(BackendError::Unsupported {
            feature: format!("Windows write to {:?}", command.control),
        }),
    }
}

#[cfg(any(windows, test))]
fn apply_output_gain_write(
    settings: &mut [u8; 40],
    command: &ControlCommand,
    gain_offsets: [usize; 2],
    state_offsets: [usize; 2],
) -> BackendResult<()> {
    let ControlValue::Decibels(gain_db) = &command.value else {
        return Err(BackendError::InvalidValue {
            control: command.control.clone(),
            reason: "output gain requires a decibel value".into(),
        });
    };
    if !gain_db.is_finite() || !(-60.0..=0.0).contains(gain_db) {
        return Err(BackendError::InvalidValue {
            control: command.control.clone(),
            reason: format!("gain {gain_db} is outside [-60, 0] dB"),
        });
    }
    let whole_db = (-60_i16..=0_i16)
        .find(|candidate| (f32::from(*candidate) - *gain_db).abs() <= f32::EPSILON)
        .ok_or_else(|| BackendError::InvalidValue {
            control: command.control.clone(),
            reason: format!("gain {gain_db} is not aligned to the 1 dB hardware step"),
        })?;
    let encoded = (whole_db * 256).to_le_bytes();
    for offset in gain_offsets {
        settings[offset..offset + 2].copy_from_slice(&encoded);
    }
    for offset in state_offsets {
        settings[offset] = u8::from(whole_db == -60);
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn apply_input_gain_write(
    settings: &mut [u8; 40],
    command: &ControlCommand,
    gain_offset: usize,
    state_offset: usize,
) -> BackendResult<()> {
    let ControlValue::Decibels(gain_db) = &command.value else {
        return Err(BackendError::InvalidValue {
            control: command.control.clone(),
            reason: "input preamp gain requires a decibel value".into(),
        });
    };
    if !gain_db.is_finite() || !(-7.0..=48.0).contains(gain_db) {
        return Err(BackendError::InvalidValue {
            control: command.control.clone(),
            reason: format!("gain {gain_db} is outside [-7, 48] dB"),
        });
    }
    let whole_db = (-7_i16..=48_i16)
        .find(|candidate| (f32::from(*candidate) - *gain_db).abs() <= f32::EPSILON)
        .ok_or_else(|| BackendError::InvalidValue {
            control: command.control.clone(),
            reason: format!("gain {gain_db} is not aligned to the 1 dB hardware step"),
        })?;
    let raw = whole_db * 256;
    settings[gain_offset..gain_offset + 2].copy_from_slice(&raw.to_le_bytes());
    settings[state_offset] = u8::from(whole_db == -7);
    Ok(())
}

#[cfg(any(windows, test))]
fn apply_boolean_settings_write(
    settings: &mut [u8; 40],
    command: &ControlCommand,
    offset: usize,
    control_name: &str,
) -> BackendResult<()> {
    let ControlValue::Boolean(enabled) = &command.value else {
        return Err(BackendError::InvalidValue {
            control: command.control.clone(),
            reason: format!("{control_name} requires a boolean value"),
        });
    };
    settings[offset] = u8::from(*enabled);
    Ok(())
}

#[cfg(any(windows, test))]
fn insert_db(controls: &mut BTreeMap<ControlId, ControlValue>, id: ControlId, value: f32) {
    controls.insert(id, ControlValue::Decibels(value));
}

#[cfg(any(windows, test))]
fn insert_bool(controls: &mut BTreeMap<ControlId, ControlValue>, id: ControlId, value: bool) {
    controls.insert(id, ControlValue::Boolean(value));
}

#[cfg(any(windows, test))]
fn decode_hardware_settings(
    settings: &[u8; 40],
) -> BackendResult<BTreeMap<ControlId, ControlValue>> {
    let mut controls = BTreeMap::new();
    decode_output_settings(settings, &mut controls)?;
    decode_input_settings(settings, &mut controls)?;
    Ok(controls)
}

#[cfg(any(windows, test))]
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

#[cfg(any(windows, test))]
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
    }

    #[cfg(windows)]
    #[test]
    fn validated_output_gain_capabilities_are_writable() {
        let capabilities = capabilities_for(CapabilityOs::Windows, Some("0x018A"), "Stream 4x5")
            .expect("embedded capability profile is valid");
        for bus in [BusId::Output1_2, BusId::Output3_4, BusId::Output5_6] {
            let descriptor = capabilities
                .descriptor(&ControlId::OutputVolume { bus })
                .expect("output descriptor exists");
            assert!(descriptor.writable);
        }
    }

    #[test]
    fn input_gain_write_preserves_every_unrelated_settings_byte() {
        let fixture = include_str!("../../../fixtures/stream4x5/vendor-api-read-v1.json");
        let response = fixture
            .split("\"response_hex\": \"")
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("fixture contains settings response");
        let mut settings: [u8; 40] = decode_hex(response)
            .try_into()
            .expect("fixture is exactly one settings block");
        let before = settings;

        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::InputVolume {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Decibels(12.0),
            },
        )
        .expect("documented input gain is writable");

        assert_eq!(&settings[18..20], &12_i16.wrapping_mul(256).to_le_bytes());
        assert_eq!(settings[26], 0);
        for index in 0..settings.len() {
            if !matches!(index, 18 | 19 | 26) {
                assert_eq!(settings[index], before[index], "byte {index} changed");
            }
        }
    }

    #[test]
    fn sanitized_write_fixture_matches_the_output_gain_encoder() {
        let fixture =
            include_str!("../../../fixtures/stream4x5/vendor-api-output-gain-write-v1.json");
        let before = fixture_string(fixture, "before_block_hex");
        let expected = fixture_string(fixture, "expected_written_block_hex");
        let mut settings: [u8; 40] = decode_hex(before)
            .try_into()
            .expect("fixture before block is exactly 40 bytes");
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::OutputVolume {
                    bus: BusId::Output3_4,
                },
                value: ControlValue::Decibels(-17.0),
            },
        )
        .expect("golden fixture command encodes");
        assert_eq!(settings.as_slice(), decode_hex(expected));
    }

    #[test]
    fn output_gain_write_changes_only_the_selected_stereo_pair_and_state_flags() {
        let mut settings = [0xA5; 40];
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::OutputVolume {
                    bus: BusId::Output5_6,
                },
                value: ControlValue::Decibels(-24.0),
            },
        )
        .expect("output gain is encoded");
        let encoded = (-24_i16 * 256).to_le_bytes();
        assert_eq!(&settings[8..10], &encoded);
        assert_eq!(&settings[10..12], &encoded);
        assert_eq!(&settings[16..18], &[0, 0]);
        for (index, value) in settings.into_iter().enumerate() {
            if !(8..12).contains(&index) && !(16..18).contains(&index) {
                assert_eq!(value, 0xA5, "byte {index} changed");
            }
        }
    }

    #[test]
    fn output_gain_endpoint_sets_and_clears_the_paired_state_flags() {
        let mut settings = [0; 40];
        for (gain, expected) in [(-60.0, [1, 1]), (-59.0, [0, 0])] {
            apply_settings_write(
                &mut settings,
                &ControlCommand {
                    control: ControlId::OutputVolume {
                        bus: BusId::Output1_2,
                    },
                    value: ControlValue::Decibels(gain),
                },
            )
            .expect("output endpoint is encoded");
            assert_eq!(&settings[12..14], &expected);
        }
    }

    #[test]
    fn output_gain_write_rejects_wrong_type_step_and_range() {
        for value in [
            ControlValue::Boolean(false),
            ControlValue::Decibels(-12.5),
            ControlValue::Decibels(-61.0),
            ControlValue::Decibels(1.0),
        ] {
            assert!(matches!(
                apply_settings_write(
                    &mut [0; 40],
                    &ControlCommand {
                        control: ControlId::OutputVolume {
                            bus: BusId::Output1_2,
                        },
                        value,
                    },
                ),
                Err(BackendError::InvalidValue { .. })
            ));
        }
    }

    #[test]
    fn sanitized_write_fixture_matches_the_input_gain_encoder() {
        let fixture =
            include_str!("../../../fixtures/stream4x5/vendor-api-input-gain-write-v1.json");
        let before = fixture_string(fixture, "before_block_hex");
        let expected = fixture_string(fixture, "expected_written_block_hex");
        let mut settings: [u8; 40] = decode_hex(before)
            .try_into()
            .expect("fixture before block is exactly 40 bytes");
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::InputVolume {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Decibels(45.0),
            },
        )
        .expect("golden fixture command encodes");
        assert_eq!(settings.as_slice(), decode_hex(expected));
    }

    #[test]
    fn sanitized_write_fixture_matches_the_high_pass_encoder() {
        let fixture =
            include_str!("../../../fixtures/stream4x5/vendor-api-high-pass-write-v1.json");
        let before = fixture_string(fixture, "before_block_hex");
        let expected = fixture_string(fixture, "expected_written_block_hex");
        let mut settings: [u8; 40] = decode_hex(before)
            .try_into()
            .expect("fixture before block is exactly 40 bytes");
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::HighPass {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Boolean(false),
            },
        )
        .expect("golden fixture command encodes");
        assert_eq!(settings.as_slice(), decode_hex(expected));
    }

    #[test]
    fn high_pass_write_changes_only_the_selected_channel_byte() {
        let mut settings = [0xA5; 40];
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::HighPass {
                    channel: ChannelId::Input2,
                },
                value: ControlValue::Boolean(true),
            },
        )
        .expect("high-pass write is supported");
        assert_eq!(settings[31], 1);
        for (index, value) in settings.into_iter().enumerate() {
            if index != 31 {
                assert_eq!(value, 0xA5, "byte {index} changed");
            }
        }
    }

    #[test]
    fn high_pass_write_rejects_non_boolean_values() {
        assert!(matches!(
            apply_settings_write(
                &mut [0; 40],
                &ControlCommand {
                    control: ControlId::HighPass {
                        channel: ChannelId::Input1,
                    },
                    value: ControlValue::Decibels(0.0),
                },
            ),
            Err(BackendError::InvalidValue { .. })
        ));
    }

    #[test]
    fn sanitized_write_fixture_matches_the_phase_invert_encoder() {
        let fixture =
            include_str!("../../../fixtures/stream4x5/vendor-api-phase-invert-write-v1.json");
        let before = fixture_string(fixture, "before_block_hex");
        let expected = fixture_string(fixture, "expected_written_block_hex");
        let mut settings: [u8; 40] = decode_hex(before)
            .try_into()
            .expect("fixture before block is exactly 40 bytes");
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::PhaseInvert {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Boolean(true),
            },
        )
        .expect("golden fixture command encodes");
        assert_eq!(settings.as_slice(), decode_hex(expected));
    }

    #[test]
    fn phase_invert_write_changes_only_the_selected_channel_byte() {
        let mut settings = [0xA5; 40];
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::PhaseInvert {
                    channel: ChannelId::Input2,
                },
                value: ControlValue::Boolean(false),
            },
        )
        .expect("phase-invert write is supported");
        assert_eq!(settings[35], 0);
        for (index, value) in settings.into_iter().enumerate() {
            if index != 35 {
                assert_eq!(value, 0xA5, "byte {index} changed");
            }
        }
    }

    #[test]
    fn phase_invert_write_rejects_non_boolean_values() {
        assert!(matches!(
            apply_settings_write(
                &mut [0; 40],
                &ControlCommand {
                    control: ControlId::PhaseInvert {
                        channel: ChannelId::Input1,
                    },
                    value: ControlValue::Integer(1),
                },
            ),
            Err(BackendError::InvalidValue { .. })
        ));
    }

    #[test]
    fn sanitized_write_fixture_matches_the_phantom_power_encoder() {
        let fixture = include_str!("../../../fixtures/stream4x5/vendor-api-phantom-write-v1.json");
        let before = fixture_string(fixture, "before_block_hex");
        let expected = fixture_string(fixture, "expected_written_block_hex");
        let mut settings: [u8; 40] = decode_hex(before)
            .try_into()
            .expect("fixture before block is exactly 40 bytes");
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::PhantomPower {
                    channel: ChannelId::Input2,
                },
                value: ControlValue::Boolean(true),
            },
        )
        .expect("golden fixture command encodes");
        assert_eq!(settings.as_slice(), decode_hex(expected));
    }

    #[test]
    fn phantom_power_write_changes_only_the_selected_channel_byte() {
        let mut settings = [0xA5; 40];
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::PhantomPower {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Boolean(false),
            },
        )
        .expect("phantom-power write is encoded");
        assert_eq!(settings[38], 0);
        for (index, value) in settings.into_iter().enumerate() {
            if index != 38 {
                assert_eq!(value, 0xA5, "byte {index} changed");
            }
        }
    }

    #[test]
    fn phantom_power_write_rejects_non_boolean_values() {
        assert!(matches!(
            apply_settings_write(
                &mut [0; 40],
                &ControlCommand {
                    control: ControlId::PhantomPower {
                        channel: ChannelId::Input1,
                    },
                    value: ControlValue::Integer(48),
                },
            ),
            Err(BackendError::InvalidValue { .. })
        ));
    }

    #[test]
    fn input_gain_endpoint_sets_the_derived_state_flag() {
        let mut settings = [0xA5; 40];
        apply_settings_write(
            &mut settings,
            &ControlCommand {
                control: ControlId::InputVolume {
                    channel: ChannelId::Input2,
                },
                value: ControlValue::Decibels(-7.0),
            },
        )
        .expect("input endpoint is valid");
        assert_eq!(&settings[20..22], &(-7_i16 * 256).to_le_bytes());
        assert_eq!(settings[27], 1);
        assert_eq!(settings[19], 0xA5);
        assert_eq!(settings[28], 0xA5);
    }

    #[test]
    fn input_gain_write_rejects_wrong_type_step_range_and_control() {
        let invalid_commands = [
            ControlCommand {
                control: ControlId::InputVolume {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Boolean(true),
            },
            ControlCommand {
                control: ControlId::InputVolume {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Decibels(1.5),
            },
            ControlCommand {
                control: ControlId::InputVolume {
                    channel: ChannelId::Input1,
                },
                value: ControlValue::Decibels(49.0),
            },
        ];
        for command in invalid_commands {
            assert!(matches!(
                apply_settings_write(&mut [0; 40], &command),
                Err(BackendError::InvalidValue { .. })
            ));
        }
        assert!(matches!(
            apply_settings_write(
                &mut [0; 40],
                &ControlCommand {
                    control: ControlId::SampleRate,
                    value: ControlValue::Integer(48_000),
                },
            ),
            Err(BackendError::Unsupported { .. })
        ));
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

    fn fixture_string<'a>(fixture: &'a str, key: &str) -> &'a str {
        fixture
            .split(&format!("\"{key}\": \""))
            .nth(1)
            .and_then(|tail| tail.split('"').next())
            .expect("fixture contains the requested string")
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
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "writes Input 1/2 gain on a connected Stream 4x5 and restores both"]
    fn live_input_gains_round_trip_restore_original_values() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        for channel in [ChannelId::Input1, ChannelId::Input2] {
            live_gain_round_trip(&mut backend, channel);
        }
        let closed = backend.close();
        closed.expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "writes all three output gains on a connected Stream 4x5 and restores exact stereo values"]
    fn live_output_gains_round_trip_restore_original_values() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        for (bus, first_gain_offset, second_gain_offset) in [
            (BusId::Output1_2, 0, 2),
            (BusId::Output3_4, 4, 6),
            (BusId::Output5_6, 8, 10),
        ] {
            live_output_gain_round_trip(&mut backend, bus, first_gain_offset, second_gain_offset);
        }
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "changes the device sample rate and requires all audio clients to be stopped"]
    fn live_sample_rate_round_trip_restores_original_value() {
        assert_eq!(
            std::env::var("LEWITT_CTL_CONFIRM_SAMPLE_RATE_SAFE").as_deref(),
            Ok("1"),
            "set LEWITT_CTL_CONFIRM_SAMPLE_RATE_SAFE=1 only after all ASIO/WASAPI clients are stopped"
        );
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        let handle = backend.require_open().expect("device is open").handle;
        let original = backend
            .ensure_api()
            .and_then(|api| api.current_sample_rate(handle))
            .expect("original sample rate is readable");
        let supported = backend
            .ensure_api()
            .and_then(|api| api.supported_sample_rates(handle))
            .expect("supported sample rates are readable");
        let temporary = supported
            .iter()
            .copied()
            .find(|sample_rate| *sample_rate != original)
            .unwrap_or_else(|| {
                panic!(
                    "device reports no alternative sample rate: current={original}, supported={supported:?}"
                )
            });

        let write = backend
            .ensure_api()
            .and_then(|api| api.set_sample_rate(handle, temporary));
        let observed = backend
            .ensure_api()
            .and_then(|api| api.current_sample_rate(handle));
        let restore = backend
            .ensure_api()
            .and_then(|api| api.set_sample_rate(handle, original));
        let restored = backend
            .ensure_api()
            .and_then(|api| api.current_sample_rate(handle));

        write.expect("temporary sample-rate write succeeds");
        assert_eq!(observed, Ok(temporary));
        restore.expect("original sample rate is restored");
        assert_eq!(restored, Ok(original));
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "writes the already-current sample rate and verifies read-back without changing it"]
    fn live_sample_rate_current_value_noop_write() {
        assert_eq!(
            std::env::var("LEWITT_CTL_CONFIRM_SAMPLE_RATE_SAFE").as_deref(),
            Ok("1"),
            "set LEWITT_CTL_CONFIRM_SAMPLE_RATE_SAFE=1 only after all ASIO/WASAPI clients are stopped"
        );
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        let handle = backend.require_open().expect("device is open").handle;
        let original = backend
            .ensure_api()
            .and_then(|api| api.current_sample_rate(handle))
            .expect("current sample rate is readable");
        let supported = backend
            .ensure_api()
            .and_then(|api| api.supported_sample_rates(handle))
            .expect("supported sample rates are readable");
        assert!(supported.contains(&original));

        backend
            .ensure_api()
            .and_then(|api| api.set_sample_rate(handle, original))
            .expect("writing the reported current sample rate succeeds");
        let observed = backend
            .ensure_api()
            .and_then(|api| api.current_sample_rate(handle))
            .expect("sample rate is readable after the no-op write");
        assert_eq!(observed, original);
        eprintln!("sample_rate_noop_write_hz={observed}");
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "loads the registered vendor API to decode the observed sample-rate rejection"]
    fn live_sample_rate_rejection_status_text() {
        let mut backend = WindowsBackend::new();
        let api = backend.ensure_api().expect("registered vendor API loads");
        for status in [0xEE00_1002, 0xEE00_1004] {
            let text = api
                .status_code_string(status)
                .expect("vendor API returns status text");
            eprintln!("0x{status:08X}={text}");
            assert_ne!(text, "");
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads and reports the current sample rate without changing hardware"]
    fn live_read_only_current_sample_rate() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        let handle = backend.require_open().expect("device is open").handle;
        let sample_rate = backend
            .ensure_api()
            .and_then(|api| api.current_sample_rate(handle))
            .expect("current sample rate is readable");
        eprintln!("current_sample_rate_hz={sample_rate}");
        assert!(matches!(sample_rate, 44_100 | 48_000 | 96_000));
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads and reports the device-supported sample-rate list without changing hardware"]
    fn live_read_only_supported_sample_rates() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        let handle = backend.require_open().expect("device is open").handle;
        let rates = backend
            .ensure_api()
            .and_then(|api| api.supported_sample_rates(handle))
            .expect("supported sample rates are readable");
        eprintln!("supported_sample_rates_hz={rates:?}");
        assert_ne!(rates.len(), 0);
        assert!(rates.iter().all(|rate| *rate > 0));
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "toggles Input 1/2 high-pass on a connected Stream 4x5 and restores both"]
    fn live_high_pass_round_trip_restores_original_values() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        for channel in [ChannelId::Input1, ChannelId::Input2] {
            live_boolean_round_trip(&mut backend, &ControlId::HighPass { channel });
        }
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "toggles Input 1/2 phase invert on a connected Stream 4x5 and restores both"]
    fn live_phase_invert_round_trip_restores_original_values() {
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        for channel in [ChannelId::Input1, ChannelId::Input2] {
            live_boolean_round_trip(&mut backend, &ControlId::PhaseInvert { channel });
        }
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires safe/disconnected inputs; toggles Input 1/2 48V and restores both"]
    fn live_phantom_power_round_trip_restores_original_values() {
        assert_eq!(
            std::env::var("LEWITT_CTL_CONFIRM_PHANTOM_SAFE").as_deref(),
            Ok("1"),
            "set LEWITT_CTL_CONFIRM_PHANTOM_SAFE=1 only after both inputs are disconnected or confirmed phantom-safe"
        );
        let mut backend = WindowsBackend::new();
        let devices = backend.enumerate().expect("enumeration succeeds");
        let device = devices.first().expect("a Stream 4x5 is connected");
        backend.open(&device.id).expect("device open succeeds");
        for channel in [ChannelId::Input1, ChannelId::Input2] {
            live_boolean_round_trip(&mut backend, &ControlId::PhantomPower { channel });
        }
        backend.close().expect("device close succeeds");
    }

    #[cfg(windows)]
    fn live_gain_round_trip(backend: &mut WindowsBackend, channel: ChannelId) {
        let control = ControlId::InputVolume { channel };
        let original = backend
            .read_back(&control)
            .expect("original gain is readable");
        let ControlValue::Decibels(original_db) = original else {
            panic!("input gain is not a decibel value");
        };
        let target_db = if original_db <= 47.0 {
            original_db + 1.0
        } else {
            original_db - 1.0
        };
        backend
            .set_control(&ControlCommand {
                control: control.clone(),
                value: ControlValue::Decibels(target_db),
            })
            .expect("temporary write succeeds");
        let observed = backend.read_back(&control);

        let restore = backend.set_control(&ControlCommand {
            control: control.clone(),
            value: ControlValue::Decibels(original_db),
        });
        let restored = backend.read_back(&control);

        assert_eq!(observed, Ok(ControlValue::Decibels(target_db)));
        restore.expect("original gain is restored");
        assert_eq!(restored, Ok(ControlValue::Decibels(original_db)));
    }

    #[cfg(windows)]
    fn live_output_gain_round_trip(
        backend: &mut WindowsBackend,
        bus: BusId,
        first_gain_offset: usize,
        second_gain_offset: usize,
    ) {
        let handle = backend.require_open().expect("device is open").handle;
        let original_settings = backend
            .ensure_api()
            .and_then(|api| api.stream4x5_settings(handle))
            .expect("original settings are readable");
        let original_first = original_settings[first_gain_offset..first_gain_offset + 2]
            .try_into()
            .map(i16::from_le_bytes)
            .expect("first output gain is present");
        let original_second = original_settings[second_gain_offset..second_gain_offset + 2]
            .try_into()
            .map(i16::from_le_bytes)
            .expect("second output gain is present");
        let original_db = f32::midpoint(
            f32::from(original_first) / 256.0,
            f32::from(original_second) / 256.0,
        );
        assert!(
            (-60.0..=0.0).contains(&original_db) && original_db.fract() == 0.0,
            "original paired output gain must be an integral value in the validated range"
        );
        let target_db = if original_db <= -1.0 {
            original_db + 1.0
        } else {
            original_db - 1.0
        };
        let control = ControlId::OutputVolume { bus };
        backend
            .set_control(&ControlCommand {
                control: control.clone(),
                value: ControlValue::Decibels(target_db),
            })
            .expect("temporary output-gain write succeeds");
        let observed = backend.read_back(&control);

        let restore = backend.ensure_api().and_then(|api| {
            let mut original_settings = original_settings;
            api.set_stream4x5_settings(handle, &mut original_settings)
        });
        let restored_settings = backend
            .ensure_api()
            .and_then(|api| api.stream4x5_settings(handle));

        assert_eq!(observed, Ok(ControlValue::Decibels(target_db)));
        restore.expect("the exact original output settings block is restored");
        let restored_settings = restored_settings.expect("restored settings are readable");
        assert_eq!(restored_settings, original_settings);
    }

    #[cfg(windows)]
    fn live_boolean_round_trip(backend: &mut WindowsBackend, control: &ControlId) {
        let original = backend
            .read_back(control)
            .expect("original boolean value is readable");
        let ControlValue::Boolean(original_enabled) = original else {
            panic!("control is not a boolean value");
        };
        let temporary = !original_enabled;
        backend
            .set_control(&ControlCommand {
                control: control.clone(),
                value: ControlValue::Boolean(temporary),
            })
            .expect("temporary write succeeds");
        let observed = backend.read_back(control);

        let restore = backend.set_control(&ControlCommand {
            control: control.clone(),
            value: ControlValue::Boolean(original_enabled),
        });
        let restored = backend.read_back(control);

        assert_eq!(observed, Ok(ControlValue::Boolean(temporary)));
        restore.expect("original boolean value is restored");
        assert_eq!(restored, Ok(ControlValue::Boolean(original_enabled)));
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
