# Windows vendor API and control-center reverse-engineering notes

This is a clean-room interface note for implementing the Windows backend. It
contains no vendor binary, decompiled source, firmware image, serial number, or
device-instance path. All offsets and signatures below came from static
analysis plus narrowly scoped read and reversible write probes.

## Analysed artifacts

| File | Architecture / kind | Version | SHA-256 |
|---|---|---|---|
| `Lewitt Control Center 2.2.exe` | x86 .NET/WPF | `2.3.0.0` | `FA865BC1A90D7473D7FA99349E6072B5257CA5E70D4FD49483B0BC99F38DC09F` |
| `Lewitt Control Center.exe` | x86 .NET/WPF | `2.3.0.0` | `B380BA3A850A70F66AD5E017F4C17B14ECA1BD6714D39C3B17D935FD518ECD08` |
| `dgtdevices.dll` | x86 managed | `2.3.0.0` | `2C43DD755E66D55034DB6F4E288C03BBD320BDDA16CC22A075E7306FFE7C1EC8` |
| `stream4x5.dll` | x86 mixed C++/CLI | `2.2.9.0` | `797E0E6E695D0240F0DDA02CE2D82576242222C77F022E6A29285735E5E6C96D` |
| `dgtstreamapi_x64.dll` | x64 native | file `4.67.0.0`, API `5.2` | `9C8A22DCA7B78224AD22347B3AC0C26C449B3A1ABC6263CDB0AE21657B3EACDA` |

The two control-center executables decompile to the same application code and
resources; the output project filename is the only source-level difference.
The signed `2.2` filename is the process observed running. No process was
terminated or modified during analysis.

## Loading and ABI rules

- Resolve CLSID `{ADACFE1D-A8E1-4606-9093-3A7418223B78}` through
  `HKCR\CLSID\{...}\InprocServer32` and load that absolute registered path.
- Never search the current directory and never redistribute the DLL.
- Require `TUSBAUDIO_CheckApiVersion(5, 2) != 0`. The observed
  `TUSBAUDIO_GetApiVersion()` value is `0x00050002`.
- The original x86 wrapper uses `__stdcall`. Rust should use `extern "system"`;
  Windows x64 has one platform calling convention.
- Handles are 32-bit integers, not pointers. A successful open handle must be
  closed exactly once.
- All request and structure buffers observed are caller-owned and synchronous;
  no pointer is retained after the call. `StatusCodeStringA` returns a borrowed
  static string and must not be freed.
- Zero-initialize opaque output structures and use the exact recovered size.
- Treat every non-zero status as failure. Do not call an unresolved export
  with a guessed prototype.

The mixed wrapper stores 66 function pointers in a 276-byte x86 loader object,
followed by the module handle and packed API version. It resolves symbols by
name, requires the base API surface, adds preferred-ASIO functions for API 5.1,
and adds device statistics for API 5.2. A Rust implementation should not copy
the C++ object's positional table: resolve a typed subset by export name. All
firmware/DFU entries are prohibited regardless of signature confidence.

The Rust Windows backend now implements this loading boundary for a strict
named subset, requires API 5.2, filters the
opened device by `VID_29C2&PID_0011` and the Stream 4x5 model string, and reads
device properties, the 40-byte settings block, sample rate, clock source, and
ASIO instance information. It resolves `AudioControlRequestSet` only for a
full-block read-modify-write of Input 1/2 preamp gain, 48V phantom power,
80 Hz high-pass, phase-invert state, the three physical output gains, and
paired hardware output mute.
Explicitly ignored hardware tests changed every input and output gain by 1 dB
and toggled each boolean state, read every value back, restored the originals,
and read the restored values on firmware `0x018A`. Output restoration also
verified the exact original Q8.8 value for all six physical channels. Raw
class/vendor, firmware, and DFU symbols remain unresolved.

## Export inventory

`exact` means the prototype was recovered from an actual caller or validated
read-only. `inventory` means name and ordinal only; it must not be called until
its prototype and ownership rules are recovered. `prohibited` is outside this
project's scope.

| Ord | Export | Status |
|---:|---|---|
| 1 | `DllRegisterServer` | inventory; installer only |
| 2 | `DllUnregisterServer` | inventory; installer only |
| 3 | `TUSBAUDIO_AudioControlRequestGet` | exact |
| 4 | `TUSBAUDIO_AudioControlRequestSet` | exact; live Input 1/2 gain, 48V, 80 Hz high-pass, phase-invert, three paired output gains, and hardware output-mute write/read-back/restore on firmware `0x018A` |
| 5 | `TUSBAUDIO_CheckApiVersion` | exact |
| 6 | `TUSBAUDIO_ClassVendorRequestIn` | inventory; no Stream 4x5 caller found |
| 7 | `TUSBAUDIO_ClassVendorRequestOut` | inventory; do not expose raw writes |
| 8 | `TUSBAUDIO_ClearPreferredASIODevice` | inventory |
| 9 | `TUSBAUDIO_CloseDevice` | exact |
| 10 | `TUSBAUDIO_EndDfuProc` | prohibited |
| 11 | `TUSBAUDIO_EnumerateDevices` | exact |
| 12 | `TUSBAUDIO_GetASIOInstanceInfo` | exact |
| 13 | `TUSBAUDIO_GetApiVersion` | exact |
| 14 | `TUSBAUDIO_GetChannelIdString` | inventory |
| 15 | `TUSBAUDIO_GetChannelIndexForChannelIdString` | inventory |
| 16 | `TUSBAUDIO_GetChannelInfo` | inventory |
| 17 | `TUSBAUDIO_GetChannelProperties` | inventory |
| 18 | `TUSBAUDIO_GetClientInfo` | inventory |
| 19 | `TUSBAUDIO_GetClockSourceStatus` | inventory |
| 20 | `TUSBAUDIO_GetCurrentClockSource` | exact |
| 21 | `TUSBAUDIO_GetCurrentSampleRate` | exact |
| 22 | `TUSBAUDIO_GetCurrentStreamFormat` | inventory |
| 23 | `TUSBAUDIO_GetDeviceContainerIdString` | inventory |
| 24 | `TUSBAUDIO_GetDeviceCount` | exact; returns a count, not a status |
| 25 | `TUSBAUDIO_GetDeviceInstanceIdString` | exact |
| 26 | `TUSBAUDIO_GetDeviceProperties` | exact |
| 27 | `TUSBAUDIO_GetDeviceStreamingMode` | inventory |
| 28 | `TUSBAUDIO_GetDeviceUsbMode` | inventory |
| 29 | `TUSBAUDIO_GetDfuStatus` | prohibited |
| 30 | `TUSBAUDIO_GetDriverInfo` | inventory |
| 31 | `TUSBAUDIO_GetDspProperty` | exact |
| 32 | `TUSBAUDIO_GetFirmwareImage` | prohibited |
| 33 | `TUSBAUDIO_GetFirmwareImageSize` | prohibited |
| 34 | `TUSBAUDIO_GetMute` | inventory |
| 35 | `TUSBAUDIO_GetStreamFormatSelectionMode` | inventory |
| 36 | `TUSBAUDIO_GetSupportedClockSources` | inventory |
| 37 | `TUSBAUDIO_GetSupportedSampleRates` | exact; read-only returned only 48000 Hz on firmware `0x018A` |
| 38 | `TUSBAUDIO_GetSupportedStreamFormats` | inventory |
| 39 | `TUSBAUDIO_GetUsbConfigDescriptor` | exact, read-only validated |
| 40 | `TUSBAUDIO_GetUsbStringDescriptorString` | inventory; may expose identifiers |
| 41 | `TUSBAUDIO_GetVolume` | inventory |
| 42 | `TUSBAUDIO_GetVolumeMuteInfo` | inventory |
| 43 | `TUSBAUDIO_LoadFirmwareImageFromBuffer` | prohibited |
| 44 | `TUSBAUDIO_LoadFirmwareImageFromFile` | prohibited |
| 45 | `TUSBAUDIO_OpenDeviceByChannelIdString` | inventory |
| 46 | `TUSBAUDIO_OpenDeviceByIndex` | exact |
| 47 | `TUSBAUDIO_QueryDeviceStatistics` | exact; read-only but contains a serial field |
| 48 | `TUSBAUDIO_QueryDriverStatistics` | inventory |
| 49 | `TUSBAUDIO_ReadDeviceNotification` | inventory |
| 50 | `TUSBAUDIO_RegisterDeviceNotification` | inventory |
| 51 | `TUSBAUDIO_RegisterPnpNotification` | inventory; callback lifetime unresolved |
| 52 | `TUSBAUDIO_ResetDriverStatistics` | inventory; mutating diagnostic state |
| 53 | `TUSBAUDIO_SetASIOBufferPreferredSize` | exact; no live write |
| 54 | `TUSBAUDIO_SetCurrentClockSource` | inventory; no live write |
| 55 | `TUSBAUDIO_SetCurrentStreamFormat` | inventory; no live write |
| 56 | `TUSBAUDIO_SetDeviceStreamingMode` | exact; original app uses it, no probe write |
| 57 | `TUSBAUDIO_SetDspProperty` | exact; test-only property-400 mute/write/read-back/exact-restore validated, public write remains disabled |
| 58 | `TUSBAUDIO_SetMute` | inventory; no live write |
| 59 | `TUSBAUDIO_SetPreferredASIODevice` | inventory; changes system driver state |
| 60 | `TUSBAUDIO_SetSampleRate` | exact; gated no-op 48000 Hz write/read-back passed, 44100 Hz rejected, no live rate change validated |
| 61 | `TUSBAUDIO_SetVolume` | inventory; no live write |
| 62 | `TUSBAUDIO_StartDfuDownload` | prohibited |
| 63 | `TUSBAUDIO_StartDfuRevertToFactoryImage` | prohibited |
| 64 | `TUSBAUDIO_StartDfuUpload` | prohibited |
| 65 | `TUSBAUDIO_StatusCodeStringA` | exact |
| 66 | `TUSBAUDIO_StatusCodeStringW` | return ownership known; prototype otherwise inventory |
| 67 | `TUSBAUDIO_UnloadFirmwareImage` | prohibited |
| 68 | `TUSBAUDIO_UnregisterPnpNotification` | inventory |

## Recovered prototypes used by Stream 4x5

Types are expressed in fixed-width C notation. On x86 the calls are
`__stdcall`; on x64 use the Windows system ABI.

```c
uint32_t GetApiVersion(void);
int32_t  CheckApiVersion(uint32_t major, uint32_t minor);
uint32_t EnumerateDevices(void);
uint32_t GetDeviceCount(void);
uint32_t OpenDeviceByIndex(uint32_t index, uint32_t *handle);
uint32_t CloseDevice(uint32_t handle);
uint32_t GetDeviceProperties(uint32_t handle, void *properties_1040);
uint32_t GetDeviceInstanceIdString(uint32_t handle,
                                   uint16_t *utf16_buffer,
                                   uint32_t max_chars);
uint32_t GetCurrentSampleRate(uint32_t handle, uint32_t *sample_rate);
uint32_t GetSupportedSampleRates(uint32_t handle, uint32_t capacity,
                                 uint32_t *sample_rates,
                                 uint32_t *returned_count);
uint32_t SetSampleRate(uint32_t handle, uint32_t sample_rate);
uint32_t GetCurrentClockSource(uint32_t handle, void *clock_source_340);
uint32_t GetASIOInstanceInfo(uint32_t asio_instance, void *info_196);
uint32_t SetASIOBufferPreferredSize(uint32_t asio_instance,
                                    uint32_t reference_sample_rate,
                                    uint32_t preferred_size,
                                    uint32_t options);
uint32_t SetDeviceStreamingMode(uint32_t handle,
                                uint32_t streaming_mode,
                                uint32_t flags);
uint32_t AudioControlRequestGet(uint32_t handle, uint8_t entity_id,
                                uint8_t request, uint8_t control_selector,
                                uint8_t channel_or_mixer_control,
                                void *block, uint32_t block_len,
                                uint32_t *bytes_transferred,
                                uint32_t timeout_ms);
uint32_t AudioControlRequestSet(/* same parameters */);
uint32_t GetDspProperty(uint32_t handle, void *block, uint32_t block_len);
uint32_t SetDspProperty(uint32_t handle, void *block, uint32_t block_len);
uint32_t QueryDeviceStatistics(uint32_t handle, void *stats,
                               uint32_t stats_size, uint32_t reset);
uint32_t GetUsbConfigDescriptor(uint32_t handle, void *buffer,
                                uint32_t capacity,
                                uint32_t *bytes_transferred);
const char *StatusCodeStringA(uint32_t status);
```

Recovered opaque layout facts:

- `DeviceProperties`: size 1040; firmware raw `u32` at offset 8; UTF-16
  description begins at offset 524.
- `ASIOInstanceInfo`: size 196; current buffer size at offset 28; supported
  count at 32; `u32` sizes begin at 36. The control center uses ASIO instance
  zero, filters sizes below 64, and passes option `0x00010000` when setting.
- `ClockSource`: size 340; type enum at offset 276. The control center treats
  type `1` as external and all other observed values as internal.
- `DeviceStatistics`: size 1364 and contains a UTF-16 serial field starting at
  offset 16. Do not log the raw structure.

## Status values

Success is zero. The API formats its own errors as `0xEE00xxxx`; useful values
confirmed through `StatusCodeStringA` include:

| Value | Symbol | Suggested backend mapping |
|---:|---|---|
| `0xEE000003` | `TSTATUS_TIMEOUT` | disconnected or protocol timeout, based on context |
| `0xEE000006` | `TSTATUS_IN_USE` | busy |
| `0xEE000007` | `TSTATUS_BUSY` | busy |
| `0xEE000023` | `TSTATUS_NO_DEVICES` | disconnected |
| `0xEE000030` | `TSTATUS_NOT_SUPPORTED` | unsupported |
| `0xEE000032` | `TSTATUS_NOT_ALLOWED` | permission/unsupported by context |
| `0xEE000033` | `TSTATUS_NOT_OPENED` | disconnected/invalid state |
| `0xEE000038` | `TSTATUS_NOT_PRESENT` | disconnected |
| `0xEE000041` | `TSTATUS_INVALID_PARAMETER` | invalid value or backend bug |
| `0xEE000042` | `TSTATUS_INVALID_LENGTH` | protocol mismatch |
| `0xEE000048` | `TSTATUS_INVALID_HANDLE` | disconnected/invalid state |
| `0xEE000060` | `TSTATUS_VERSION_MISMATCH` | protocol mismatch |
| `0xEE000064` | `TSTATUS_UNEXPECTED_DEVICE_RESPONSE` | protocol mismatch |
| `0xEE000071` | `TSTATUS_DEVICE_REMOVED` | disconnected |
| `0xEE000074` | `TSTATUS_BUFFER_TOO_SMALL` | backend bug/protocol mismatch |
| `0xEE000100` | `TSTATUS_INTERFACE_USED` | busy |
| `0xEE001002` | `TSTATUS_ASIO_IN_USE` | busy |
| `0xEE001004` | `TSTATUS_INVALID_SAMPLE_RATE` | invalid sample-rate value/state |

Mapping must also consider the operation; do not collapse every non-zero code
to `Disconnected`.

## DSP property transport (Windows-only)

`GetDspProperty` and `SetDspProperty` use driver IOCTLs `0x80882200` and
`0x80882204`. The INF installs `dgtstream_mixer_ducker.sys` as an upper filter.
Static analysis of that exact signed filter confirms that these structures are
plugin ABI, not USB protocol: its processing callbacks operate directly on
host PCM buffers. See
[`windows-filter-driver.md`](windows-filter-driver.md) for the filter call graph,
property validation, and meter algorithm.

Product scope is narrower than the recovered ABI. The Windows v1 UI does not
expose Ducker, compressor, equalizer, or reverb controls. Their layouts remain
documented for interoperability analysis and must not be treated as an
implementation requirement or permission to bind the write calls.

All structures are packed and little-endian. Channel type is
`APPLICATION=0`, `DEVICE=1`, `VIRTUAL=2`; meter type is `PEAK=0`, `RMS=1`;
meter position is `INPUT=0`, `PREFADER=1`, `OUTPUT=2`, `FX1=3`, `FX2=4`,
`FX3=5`.

### Mixer properties (`signature = 77`, ASCII `M`)

| Property | Size | Packed layout |
|---:|---:|---|
| 200, matrix weight | 16 | `u32 sig; u32 prop; u8 in_type; u8 in_index; u8 out_type; u8 out_index; i32 q8_24` |
| 400, output attenuation | 14 | `u32 sig; u32 prop; u8 type; u8 index; i32 q8_24` |
| 401, meter active | 20 | Same four selector bytes as property 402; a 32-bit active value begins at offset 12 |
| 402, meter | 20 | `u32 sig; u32 prop; u8 type; u8 index; u8 meter_type; u8 meter_pos; i32 q8_24; u32 processed_sample_cursor` |
| 500, performance counters | 68 | Header plus plugin timing counters; diagnostic only |

Q8.24 conversion is `raw = linear * 16777216`; reads multiply by `2^-24`.
The UI converts fader dB to linear before sending matrix or attenuation values.
The visible physical-output mute buttons are part of this attenuation path,
not the private 40-byte device settings block. Recovered IL stores the three
pair states in `out2Mute`, `out3Mute`, and `out4Mute`; `UpdateAttenuationHW`
passes exact zero for both channels of a muted pair. A live read of property
400 for `DEVICE` indices `0..5` returned
`[0, 0, 16720723, 16720723, 0, 0]` in Q8.24 while the official UI showed mute
on/off/on for output pairs 1/2, 3/4, and 5/6 respectively. A later test-only
property-400 SET probe selected one currently non-muted pair, wrote exact zero
to both channels, read both back, restored both original Q8.24 values, and
verified the restoration. The public backend does not use this DSP mute path:
recovered control-center logic derives its unmute value from routing selection,
two master attenuation/mute states, solo state, and the optional output pad,
none of which can be reconstructed from a pair that was already zero when this
application started. Public `OutputMute` instead uses the independently tested
paired device flags at settings offsets `12..17`; this avoids modifying or
guessing host-filter attenuation.
The meter cursor advances by the PCM sample count passed to the filter, not by
USB packets. Peak is accumulated from absolute signed 32-bit PCM magnitudes;
RMS uses an approximately 40 ms square/square-root window. Unread meter records
are disabled after about five seconds of processed audio and re-enabled by a
read. These are Windows host-driver meters.

### Ducker properties (`signature = 68`, ASCII `D`)

Scalar block, 13 bytes:

```text
offset 0  u32 signature
offset 4  u32 property_id
offset 8  u8  instance
offset 9  4-byte union (bool/u32/i32 Q8.24)
```

Channel block, 12 bytes:

```text
offset 0  u32 signature
offset 4  u32 property_id (10 key, 11 input)
offset 8  u8  instance
offset 9  u8  channel_type
offset 10 u8  first_index
offset 11 u8  channel_count
```

| ID | Name | Value form |
|---:|---|---|
| 1 | Enabled | 32-bit boolean |
| 2 | DelayInterval | `u32` milliseconds |
| 3 | AttackInterval | `u32` milliseconds |
| 4 | HoldInterval | `u32` milliseconds |
| 5 | ReleaseInterval | `u32` milliseconds |
| 6 | Threshold | signed Q8.24 linear gain |
| 7 | Depth | signed Q8.24 linear gain |
| 8 | LowPassCutoffFreq | `u32` Hz |
| 9 | CurrentGain | signed Q8.24 linear gain, read-oriented |
| 10 | KeyChannel | channel block |
| 11 | InputChannel | channel block |
| 12 | InstanceCount | scalar; not used by the UI path |
| 13 | KeySignalIsVirtPlayback | 32-bit boolean |
| 14 | DetectorOutputIsVirtRecord | 32-bit boolean |

The control-center UI has three Ducker instances. Hold is clipped to
`0..3000 ms`. Its response knob (`0..10`) is linearly interpolated between:

```text
value  delay_ms  attack_ms  release_ms
0      0         20         60
2      0         40         500
4      0         60         1000
6      0         100        2000
8      0         200        2000
10     200       500        4000
```

Threshold and depth are converted from dB to linear before Q8.24 packing.

### FX properties (`signature = 70`, ASCII `F`)

The global/enable forms are 12 or 13 bytes. The parameter form is 22 bytes:

```text
offset 0  u32    signature
offset 4  u32    property (9 for PSP parameter)
offset 8  u8     instance
offset 9  u8     fx_type (0 compressor, 1 equalizer, 2 reverb)
offset 10 i32    parameter key
offset 14 f64    value
```

Recovered numeric key/range mappings:

| UI property | Type/key | Accepted UI range |
|---|---:|---:|
| Compressor attack | `0/5` | `0.5..200 ms` |
| Compressor release | `0/6` | `10..4000 ms` |
| Compressor threshold | `0/11` | `-40..20 dB` |
| Compressor ratio | `0/10` | `1..16` |
| Compressor makeup | `0/12` | `-6..30 dB` |
| EQ low cut | `1/26` | `10..4000 Hz` |
| EQ band 1 frequency/gain/Q | `1/7`, `1/9`, `1/8` | `20..800 Hz`, `-20..20 dB`, shelf-dependent |
| EQ band 2 frequency/gain/Q | `1/12`, `1/14`, `1/13` | `50..2500 Hz`, `-20..20 dB`, `0.2..4` |
| EQ band 3 frequency/gain/Q | `1/17`, `1/19`, `1/18` | `240..12000 Hz`, `-20..20 dB`, `0.2..4` |
| EQ band 4 frequency/gain/Q | `1/22`, `1/24`, `1/23` | `500..20000 Hz`, `-20..20 dB`, shelf-dependent |
| Reverb mode | `2/4` | `0..2` |
| Reverb mix | `2/15` | `0..100` |
| Reverb pre-delay | `2/5` | `0..1000 ms` |
| Reverb time | `2/9` | `0..100` |
| Reverb damp | `2/10` | `1000..16000 Hz` |
| Reverb modulation | `2/11` | `0..100` |
| Reverb width | `2/12` | `0..200` |
| Reverb low cut | `2/13` | `10..1000 Hz` |
| Reverb presence | `2/14` | `0..12 dB` |

FX is outside the current v1 control model. This table documents the Windows
plugin ABI only and is not a Linux USB mapping.

## Control-center sequencing and hazards

On a one-device list change, the original application performs:

```text
LoadSettingsFromDevice
LoadSettingsFromStorage
SaveSettingsToDevice
SetDeviceStreamingMode(always-on)
ApplyPreset (mixer/Ducker/FX writes)
```

`Stream4x5.Open(reload=true)` also reads the device, overlays registry state,
and writes it back. Its settings writer zero-initializes and rewrites the whole
40-byte block, duplicates paired channels, then separately changes sample rate
and ASIO buffer size.

This is evidence about the original program, not behaviour to copy. The new
backend must never write on enumeration/open, must preserve unknown bytes,
must serialize calls on the controller worker, and must read back every write.
If the control center is running, warn; on conflict, stop writes and refresh.

## Remaining ABI work before additional Windows writes

- Recover only the additional export prototypes actually needed by the Rust
  backend; leave all other inventory entries unresolved.
- Validate x64 packing with compile-time size/offset assertions.
- Keep the implemented safe loader restricted to the registered absolute path,
  API 5.2, and the exact named-symbol subset; add driver-version policy before
  any additional write work.
- Validate one DSP read against the filter-driver layouts in
  [`windows-filter-driver.md`](windows-filter-driver.md); static recovery alone
  does not approve a runtime call.
- Capture one additional property write at a time and add a sanitized golden fixture.
- Test close/disconnect/callback lifetime and simultaneous control-center use.
- Never bind or expose DFU, firmware, raw class/vendor, or unchecked DSP calls.
