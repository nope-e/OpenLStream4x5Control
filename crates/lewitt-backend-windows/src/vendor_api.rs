#![allow(unsafe_code)]

use lewitt_core::{BackendError, BackendResult};
use libloading::Library;
use std::ffi::c_void;
use std::path::Path;

const REQUIRED_API_VERSION: u32 = 0x0005_0002;

type GetApiVersionFn = unsafe extern "system" fn() -> u32;
type CheckApiVersionFn = unsafe extern "system" fn(u32, u32) -> i32;
type EnumerateDevicesFn = unsafe extern "system" fn() -> u32;
type GetDeviceCountFn = unsafe extern "system" fn() -> u32;
type OpenDeviceByIndexFn = unsafe extern "system" fn(u32, *mut u32) -> u32;
type CloseDeviceFn = unsafe extern "system" fn(u32) -> u32;
type GetDevicePropertiesFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
type GetDeviceInstanceIdStringFn = unsafe extern "system" fn(u32, *mut u16, u32) -> u32;
type GetCurrentSampleRateFn = unsafe extern "system" fn(u32, *mut u32) -> u32;
type GetCurrentClockSourceFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
type GetAsioInstanceInfoFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
type GetDspPropertyFn = unsafe extern "system" fn(u32, *mut c_void, u32) -> u32;
type AudioControlRequestGetFn =
    unsafe extern "system" fn(u32, u8, u8, u8, u8, *mut c_void, u32, *mut u32, u32) -> u32;

pub(super) struct VendorApi {
    get_api_version: GetApiVersionFn,
    check_api_version: CheckApiVersionFn,
    enumerate_devices: EnumerateDevicesFn,
    get_device_count: GetDeviceCountFn,
    open_device_by_index: OpenDeviceByIndexFn,
    close_device: CloseDeviceFn,
    get_device_properties: GetDevicePropertiesFn,
    get_device_instance_id_string: GetDeviceInstanceIdStringFn,
    get_current_sample_rate: GetCurrentSampleRateFn,
    get_current_clock_source: GetCurrentClockSourceFn,
    get_asio_instance_info: GetAsioInstanceInfoFn,
    get_dsp_property: GetDspPropertyFn,
    audio_control_request_get: AudioControlRequestGetFn,
    _library: Library,
}

impl VendorApi {
    pub(super) fn load(path: &Path) -> BackendResult<Self> {
        if !path.is_absolute() {
            return Err(BackendError::ProtocolMismatch {
                details: "vendor API path must be absolute before loading".into(),
            });
        }

        // SAFETY: The path came from the validated 64-bit InprocServer32
        // registration. We never fall back to the working directory, and the
        // Library is retained for at least as long as every copied symbol.
        let library =
            unsafe { Library::new(path) }.map_err(|error| BackendError::DriverMissing {
                details: format!("cannot load the registered vendor API: {error}"),
            })?;

        let api = Self {
            get_api_version: load_symbol(&library, b"TUSBAUDIO_GetApiVersion\0")?,
            check_api_version: load_symbol(&library, b"TUSBAUDIO_CheckApiVersion\0")?,
            enumerate_devices: load_symbol(&library, b"TUSBAUDIO_EnumerateDevices\0")?,
            get_device_count: load_symbol(&library, b"TUSBAUDIO_GetDeviceCount\0")?,
            open_device_by_index: load_symbol(&library, b"TUSBAUDIO_OpenDeviceByIndex\0")?,
            close_device: load_symbol(&library, b"TUSBAUDIO_CloseDevice\0")?,
            get_device_properties: load_symbol(&library, b"TUSBAUDIO_GetDeviceProperties\0")?,
            get_device_instance_id_string: load_symbol(
                &library,
                b"TUSBAUDIO_GetDeviceInstanceIdString\0",
            )?,
            get_current_sample_rate: load_symbol(&library, b"TUSBAUDIO_GetCurrentSampleRate\0")?,
            get_current_clock_source: load_symbol(&library, b"TUSBAUDIO_GetCurrentClockSource\0")?,
            get_asio_instance_info: load_symbol(&library, b"TUSBAUDIO_GetASIOInstanceInfo\0")?,
            get_dsp_property: load_symbol(&library, b"TUSBAUDIO_GetDspProperty\0")?,
            audio_control_request_get: load_symbol(
                &library,
                b"TUSBAUDIO_AudioControlRequestGet\0",
            )?,
            _library: library,
        };

        // SAFETY: These functions take no pointers and their exact system-ABI
        // prototypes were recovered from callers and verified read-only.
        let (version, compatible) =
            unsafe { ((api.get_api_version)(), (api.check_api_version)(5, 2) != 0) };
        if version != REQUIRED_API_VERSION || !compatible {
            return Err(BackendError::ProtocolMismatch {
                details: format!(
                    "vendor API 5.2 is required, registered DLL reports {}.{}",
                    version >> 16,
                    version & 0xFFFF
                ),
            });
        }

        Ok(api)
    }

    pub(super) fn enumerate(&self) -> BackendResult<u32> {
        // SAFETY: Exact zero-argument ABI; the retained Library owns the code.
        let status = unsafe { (self.enumerate_devices)() };
        check_status("enumerate devices", status)?;
        // SAFETY: Exact zero-argument ABI returning a count, not a status.
        Ok(unsafe { (self.get_device_count)() })
    }

    pub(super) fn open(&self, index: u32) -> BackendResult<u32> {
        let mut handle = 0_u32;
        // SAFETY: `handle` is valid writable storage for the duration of this
        // synchronous call; the function retains no pointer.
        let status = unsafe { (self.open_device_by_index)(index, &raw mut handle) };
        check_status("open device", status)?;
        Ok(handle)
    }

    pub(super) fn close(&self, handle: u32) -> BackendResult<()> {
        // SAFETY: The handle was returned by this Library's open function and
        // the backend guarantees close is attempted at most once per handle.
        let status = unsafe { (self.close_device)(handle) };
        check_status("close device", status)
    }

    pub(super) fn device_properties(&self, handle: u32) -> BackendResult<[u8; 1040]> {
        let mut bytes = [0_u8; 1040];
        // SAFETY: The exact ABI requires a caller-owned 1040-byte output block;
        // the pointer remains valid and is not retained after the call.
        let status =
            unsafe { (self.get_device_properties)(handle, bytes.as_mut_ptr().cast::<c_void>()) };
        check_status("read device properties", status)?;
        Ok(bytes)
    }

    pub(super) fn device_instance_id(&self, handle: u32) -> BackendResult<String> {
        let mut utf16 = [0_u16; 512];
        // SAFETY: The buffer contains 512 writable UTF-16 code units, exactly
        // matching `max_chars`, and the synchronous call retains no pointer.
        let status =
            unsafe { (self.get_device_instance_id_string)(handle, utf16.as_mut_ptr(), 512) };
        check_status("read device instance ID", status)?;
        let length = utf16
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(utf16.len());
        String::from_utf16(&utf16[..length]).map_err(|error| BackendError::ProtocolMismatch {
            details: format!("device instance ID is not valid UTF-16: {error}"),
        })
    }

    pub(super) fn current_sample_rate(&self, handle: u32) -> BackendResult<u32> {
        let mut sample_rate = 0_u32;
        // SAFETY: `sample_rate` is valid writable storage and is not retained.
        let status = unsafe { (self.get_current_sample_rate)(handle, &raw mut sample_rate) };
        check_status("read sample rate", status)?;
        Ok(sample_rate)
    }

    pub(super) fn current_clock_source(&self, handle: u32) -> BackendResult<[u8; 340]> {
        let mut bytes = [0_u8; 340];
        // SAFETY: The exact ABI requires a caller-owned 340-byte output block.
        let status =
            unsafe { (self.get_current_clock_source)(handle, bytes.as_mut_ptr().cast::<c_void>()) };
        check_status("read clock source", status)?;
        Ok(bytes)
    }

    pub(super) fn asio_instance_info(&self) -> BackendResult<[u8; 196]> {
        let mut bytes = [0_u8; 196];
        // SAFETY: Instance zero is the control-center read path and `bytes` is
        // the exact caller-owned 196-byte structure required by API 5.2.
        let status =
            unsafe { (self.get_asio_instance_info)(0, bytes.as_mut_ptr().cast::<c_void>()) };
        check_status("read ASIO instance information", status)?;
        Ok(bytes)
    }

    pub(super) fn output_attenuation_raw(
        &self,
        handle: u32,
        channel_type: u8,
        channel_index: u8,
    ) -> BackendResult<i32> {
        let mut block = [0_u8; 14];
        block[0..4].copy_from_slice(&77_u32.to_le_bytes());
        block[4..8].copy_from_slice(&400_u32.to_le_bytes());
        block[8] = channel_type;
        block[9] = channel_index;
        // SAFETY: Mixer property 400 is a synchronous 14-byte GET. The
        // caller-owned packed block has the recovered signature, property,
        // channel type, and channel index fields and is not retained.
        let status =
            unsafe { (self.get_dsp_property)(handle, block.as_mut_ptr().cast::<c_void>(), 14) };
        check_status("read output attenuation", status)?;
        Ok(i32::from_le_bytes([
            block[10], block[11], block[12], block[13],
        ]))
    }

    pub(super) fn stream4x5_settings(&self, handle: u32) -> BackendResult<[u8; 40]> {
        let mut bytes = [0_u8; 40];
        let mut transferred = 0_u32;
        // SAFETY: This is the documented read-only Stream 4x5 request. The
        // caller-owned 40-byte block and transfer-count pointer remain valid
        // throughout the synchronous call and are not retained.
        let status = unsafe {
            (self.audio_control_request_get)(
                handle,
                0x33,
                0x03,
                0,
                0,
                bytes.as_mut_ptr().cast::<c_void>(),
                40,
                &raw mut transferred,
                500,
            )
        };
        check_status("read Stream 4x5 settings", status)?;
        if transferred != 40 {
            return Err(BackendError::ProtocolMismatch {
                details: format!("Stream 4x5 settings returned {transferred} bytes instead of 40"),
            });
        }
        Ok(bytes)
    }
}

fn load_symbol<T>(library: &Library, name: &'static [u8]) -> BackendResult<T>
where
    T: Copy,
{
    // SAFETY: Every requested symbol has an exact recovered prototype. The
    // copied function pointer cannot outlive `library` because VendorApi owns
    // the Library alongside all pointers.
    let symbol = unsafe { library.get::<T>(name) }.map_err(|error| {
        let printable = String::from_utf8_lossy(name)
            .trim_end_matches('\0')
            .to_owned();
        BackendError::ProtocolMismatch {
            details: format!("required vendor API export {printable} is missing: {error}"),
        }
    })?;
    Ok(*symbol)
}

fn check_status(operation: &str, status: u32) -> BackendResult<()> {
    if status == 0 {
        return Ok(());
    }
    let details = format!("{operation} failed with vendor status 0x{status:08X}");
    match status {
        0xEE00_0006 | 0xEE00_0007 | 0xEE00_0100 => Err(BackendError::Busy { details }),
        0xEE00_0023 | 0xEE00_0033 | 0xEE00_0038 | 0xEE00_0048 | 0xEE00_0071 => {
            Err(BackendError::Disconnected)
        }
        0xEE00_0032 => Err(BackendError::PermissionDenied { details }),
        0xEE00_0030 => Err(BackendError::Unsupported { feature: details }),
        _ => Err(BackendError::ProtocolMismatch { details }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_busy_statuses_without_collapsing_them_to_disconnect() {
        assert!(matches!(
            check_status("test", 0xEE00_0100),
            Err(BackendError::Busy { .. })
        ));
        assert_eq!(
            check_status("test", 0xEE00_0071),
            Err(BackendError::Disconnected)
        );
    }
}
