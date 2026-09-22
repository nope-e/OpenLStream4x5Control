# Stream 4x5 USB protocol evidence

This document is the protocol gate for the Linux backend. It describes only
Lewitt Stream 4x5 (`VID 0x29C2`, `PID 0x0011`). It is not permission to issue
writes: a control remains `Unsupported` until its write, read-back, failure
behaviour, and coexistence with `snd-usb-audio` have been captured and tested.

The Windows ABI is documented separately in
[`windows-vendor-api.md`](windows-vendor-api.md).

## Evidence levels

| Level | Meaning |
|---|---|
| S | Recovered by static analysis of the installed 2.3.0 control center, `stream4x5.dll`, vendor API, and driver package. |
| R | Repeated successfully as a read-only call against one connected Stream 4x5 through the registered 64-bit vendor API. |
| W | Written through the registered Windows vendor API, read back, restored, and read back again on the connected Stream 4x5. |
| U | Confirmed as the actual USB setup packet by USBPcap or direct Linux `libusb`/`rusb` observation. |

The findings below have S and, where stated, R or W evidence. There is no U evidence
yet. In particular, the vendor API can transform a request before it reaches
the USB bus, so a successful Windows API call is not by itself a Linux-ready
golden capture.

## Evidence snapshot

- Analysis date: 2026-09-22.
- Vendor API version returned by `TUSBAUDIO_GetApiVersion`: `5.2`
  (`0x00050002`).
- Installed API/driver file version: `4.67.0.0`.
- Connected test firmware reported by `GetDeviceProperties`: `0x018A`.
- Device enumeration, open, close, configuration-descriptor read, current
  sample-rate read, clock reads, and the three private read operations below
  succeeded.
- Input 1 and Input 2 preamp gain were each changed temporarily by 1 dB, and
  both 48V phantom-power, 80 Hz high-pass, and phase-invert states were
  temporarily inverted through the Windows vendor API after input safety was
  confirmed. All three physical output pairs were also changed temporarily by
  1 dB. Every value was read back and restored; all six original per-channel
  output values were restored exactly. One currently non-muted Windows DSP
  output pair was also muted, read back, and restored exactly through property
  400. No sample-rate, mixer-matrix, firmware, or DFU write was performed.
- Serial number and Windows device-instance ID were intentionally discarded.

Firmware `0x018A` is approved only for the Windows vendor-API Input 1/2 preamp
gain, 48V phantom-power, 80 Hz high-pass, phase-invert, and three paired
physical output-gain operations validated below. It remains read-only for
every other property. Other firmware versions remain fully read-only.

## USB configuration

The live configuration descriptor is 297 bytes and reports four interfaces:

| Interface | Alternate | Class | Observed role |
|---:|---:|---|---|
| 0 | 0 | AudioControl, UAC2 (`01/01/20`) | Clock, terminals, and feature units; no endpoint. |
| 1 | 0/1 | AudioStreaming, UAC2 | Six-channel playback, 32-bit subslot / 24-bit sample, isochronous OUT endpoint `0x01`. |
| 2 | 0/1 | AudioStreaming, UAC2 | Four-channel capture, 32-bit subslot / 24-bit sample, isochronous IN endpoint `0x81`. |
| 3 | 0 | DFU runtime (`FE/01/01`) | DFU 1.10, transfer size 64. Explicitly out of project scope. |

There is no separate ASIO endpoint. ASIO is a Windows host-driver/API path over
the same physical UAC2 playback and capture endpoints. Different host stacks
can still have different scheduling and buffering latency, so sharing the USB
endpoint pair is not by itself a latency guarantee.

The AudioControl topology contains:

| Entity | Type | Important fields |
|---:|---|---|
| `0x29` | Clock source | Internal/programmable clock; frequency is read/write and validity is read-only in the descriptor. |
| `0x28` | Clock selector | One input, sourced from `0x29`. |
| `0x02` | USB-streaming input terminal | Six playback channels. |
| `0x0A` | Playback feature unit | Master plus six channels; control bitmap `0x0000000F`. |
| `0x14` | Speaker output terminal | Source `0x0A`. |
| `0x01` | Microphone input terminal | Four capture channels. |
| `0x0B` | Capture feature unit | Master plus four channels; control bitmap `0x00000005`. |
| `0x16` | USB-streaming output terminal | Source `0x0B`. |

The private entity `0x33` used by the control center is **not present** in the
published class-specific descriptor. It is therefore a hidden firmware
contract, not a normal discoverable UAC2 unit.

## Control-request mapping

Static analysis of `TUSBAUDIO_AudioControlRequestGet/Set` gives this setup
packet mapping:

```text
GET: bmRequestType = 0xA1  (device-to-host, class, interface)
SET: bmRequestType = 0x21  (host-to-device, class, interface)
bRequest            = request
wValue              = (control_selector << 8) | channel_or_mixer_control
wIndex              = (entity_id << 8) | interface_number
interface_number    = 0 for all Stream 4x5 calls observed here
wLength             = parameter-block length
```

The wrapper uses a 500 ms timeout for the Stream 4x5 private blocks. On
Windows these calls pass through driver IOCTLs `0x80882086` (GET) and
`0x80882081` (SET). This mapping has S evidence; the logical reads in the next
section also have R evidence.

## Private entity `0x33`

All three operations use `bRequest = 0x03`, selector `0`, interface `0`:

| Operation | Direction | `wValue` | `wIndex` | Length | Evidence |
|---|---|---:|---:|---:|---|
| Settings | GET / SET | `0x0000` | `0x3300` | 40 | S+R for GET; S+W for Input 1/2 gain, 48V, 80 Hz high-pass, phase-invert, paired output gain, and hardware output-mute SET |
| Status | GET | `0x0001` | `0x3300` | 20 | S+R |
| Persistent configuration | GET / SET | `0x0002` | `0x3300` | 64 | S+R for GET; S only for SET |

### Settings block, 40 bytes

All multi-byte values are little-endian.

| Offset | Size | Meaning | Encoding / notes |
|---:|---:|---|---|
| 0 | 2 | Output 1 gain | Signed Q8.8 dB |
| 2 | 2 | Output 2 gain | Signed Q8.8 dB |
| 4 | 2 | Output 3 gain | Signed Q8.8 dB |
| 6 | 2 | Output 4 gain | Signed Q8.8 dB |
| 8 | 2 | Output 5 gain | Signed Q8.8 dB |
| 10 | 2 | Output 6 gain | Signed Q8.8 dB |
| 12 | 6 | Hardware output-mute / gain-endpoint flags 1..6 | Paired Boolean flags. Independently setting a pair to `1` physically muted that output without changing its Q8.8 gain. The recovered serializer also sets them at exactly `-60 dB` (`0xC400`) and clears them above that endpoint. They are not the control center's visible DSP output-mute buttons. |
| 18 | 2 | Input 1 gain | Signed Q8.8 dB |
| 20 | 2 | Input 2 gain | Signed Q8.8 dB |
| 22 | 2 | Input 3 stored level field | Signed Q8.8-like value; not exposed as hardware gain by the official UI. |
| 24 | 2 | Input 4 stored level field | Signed Q8.8-like value; not exposed as hardware gain by the official UI. |
| 26 | 4 | Input state flags 1..4 | One byte per channel; observed as `0` or `1`. The official UI has no independent device/preamp mute buttons; its visible strip mutes are matrix-relative. |
| 30 | 4 | High-pass 1..4 | One byte per channel; UI exposes inputs 1/2 only (`0` flat, `1` 80 Hz). |
| 34 | 4 | Phase invert 1..4 | One byte per channel; UI exposes inputs 1/2 only. |
| 38 | 1 | Input 1 phantom power | Boolean byte |
| 39 | 1 | Input 2 phantom power | Boolean byte |

Conversion is `raw_i16 = dB * 256` and `dB = raw_i16 / 256`. The control
center averages paired output values on read and duplicates its UI value on
write for outputs 1/2, 3/4, and 5/6. Recovered code also contains pairing
logic for the stored Input 3/4 fields, but the official UI does not expose
hardware gain or mute controls for Input 3/4. Those fields therefore remain
structural observations and are not public backend capabilities.

The Windows backend now exposes Input 1/2 preamp gain as writable in 1 dB
steps on firmware `0x018A`. It first reads the complete 40-byte block, changes
only offsets `18..19` plus `26` for Input 1 or `20..21` plus `27` for Input 2,
then writes the complete block. `-7 dB` sets the corresponding endpoint state
byte to `1`; all higher values clear it. A live test changed both inputs by
1 dB, verified both read-backs, restored the original values, and verified the
restored values. The sanitized encoding and validation record is
[`vendor-api-input-gain-write-v1.json`](../../fixtures/stream4x5/vendor-api-input-gain-write-v1.json).
This is Windows API evidence, not USB packet evidence; it does not approve a
Linux write implementation.

The three physical output gains are writable in 1 dB steps from `-60` through
`0 dB` on firmware `0x018A`. Output 1/2 uses offsets `0..3`, Output 3/4 uses
`4..7`, and Output 5/6 uses `8..11`; the same signed Q8.8 value is written to
both channels of the selected pair. The full-block read-modify-write leaves
the selected pair's hardware-mute flags synchronized with the vendor
serializer: `-60 dB` sets both flags and every higher gain clears them. A live
test changed each pair by 1 dB, verified the paired read-back, then restored
and verified the exact original Q8.8 value of every individual output channel.
The sanitized record is
[`vendor-api-output-gain-write-v1.json`](../../fixtures/stream4x5/vendor-api-output-gain-write-v1.json).

The same read-modify-write path now exposes Input 1/2 80 Hz high-pass on
firmware `0x018A`. It changes only byte `30` for Input 1 or byte `31` for Input
2 (`0` flat, `1` enabled). A live test inverted both values, verified both
read-backs, restored the originals, and verified the restored values. The
sanitized encoding and validation record is
[`vendor-api-high-pass-write-v1.json`](../../fixtures/stream4x5/vendor-api-high-pass-write-v1.json).

Input 1/2 phase inversion uses the same validated path and changes only byte
`34` for Input 1 or byte `35` for Input 2 (`0` normal polarity, `1` inverted).
A live test inverted both values, verified both read-backs, restored the
originals, and verified the restored values. The sanitized encoding and
validation record is
[`vendor-api-phase-invert-write-v1.json`](../../fixtures/stream4x5/vendor-api-phase-invert-write-v1.json).

Input 1/2 48V phantom power uses bytes `38` and `39` (`0` off, `1` on). After
both physical inputs were confirmed disconnected or phantom-safe, a live test
inverted both values, verified both read-backs, restored the originals, and
verified the restored values. The capability is writable only on the approved
firmware profile. The sanitized record is
[`vendor-api-phantom-write-v1.json`](../../fixtures/stream4x5/vendor-api-phantom-write-v1.json).

The official UI exposes a DSP output-mute button, but those buttons are **not**
bytes `12..17` in this device block. In a simultaneous read-only
observation, the official UI showed output pairs 1/2 and 5/6 muted and 3/4
unmuted while all six bytes at `12..17` were zero. Later static analysis of
`Stream4x5.SaveSettingsToDevice` resolved the six bytes: each paired flag is a
derived `-60 dB` hardware-gain endpoint marker. Later live testing established
that the same flags also independently gate the physical hardware output. They
still do not represent the control center's DSP mute state.

Static IL analysis identifies the visible physical-output button path as
`Click_buttonMute -> Mixer8x6.UpdateAttenuation -> UpdateAttenuationHW ->
SetOutAttenuationSafe`. The three physical pairs use preset fields `out2Mute`,
`out3Mute`, and `out4Mute`; a muted pair is sent as exact zero attenuation for
both channels through DSP property 400. A live `TUSBAUDIO_GetDspProperty` read
for channel type `DEVICE` (`1`), indices `0..5`, returned Q8.24 values
`[0, 0, 16720723, 16720723, 0, 0]`, matching the visible on/off/on state. The
DSP property 400 therefore remains useful evidence for the control center's
host-filter state, but it is not the public hardware-mute implementation.

A test-only binding of `TUSBAUDIO_SetDspProperty` has now validated the 14-byte
property-400 SET ABI. The test selected one currently non-muted pair, saved
both exact Q8.24 values, wrote zero to both channels, verified both read-backs,
then restored and verified both original values. The sanitized record is
[`vendor-api-output-mute-write-v1.json`](../../fixtures/stream4x5/vendor-api-output-mute-write-v1.json).
This proves reversible DSP mute only when a non-zero baseline has already been
captured; it does not approve using property 400 as a public write capability.

Recovered `UpdateAttenuationHW` logic shows why a constant unmute value is not
safe. A pair's non-muted attenuation depends on its current `OutSelectionHW`
routing, monitor/output master attenuation and mute state, solo mode, and its
optional `-20 dB` pad. An application that starts while the pair is already
muted cannot recover those inputs from property 400 alone. The production
backend therefore does not use property 400 for public output mute and never
guesses unity or silently changes the effective level.

Property 400 is Windows host-filter state for a physical output path, not a
device-resident USB mute. It cannot be implemented on Linux by copying that
property block to EP0. The independently validated flags at bytes `12..17` are
device-side hardware mute, but there is still no captured Linux USB SET packet
or `rusb` hardware validation. The Linux backend must therefore continue to
omit `OutputMute` from its capabilities, and the shared GUI hides the control
instead of synthesizing an off state.

The six device-block flags passed a separate hardware-effect experiment. A
gated, ignored test chooses one physical output pair whose
gain is above `-60 dB`, whose two flags are zero, and whose Windows DSP
attenuation is non-zero. It changes only that pair's two flags for five
seconds, reads the block back, then restores and verifies the exact original
40 bytes. With the control center closed, the test selected Output 1/2, read
both temporary flags back as `1`, returned `OutputMute=true` through the public
snapshot decoder, and restored an exact byte-for-byte match. The user
confirmed that Output 1/2 was physically silent during the five-second window.
The Windows firmware `0x018A` profile consequently exposes all three paired
flags as writable hardware output mute; each write changes only its two Boolean
bytes and is followed by controller read-back. The sanitized record is
[`vendor-api-output-state-probe-v1.json`](../../fixtures/stream4x5/vendor-api-output-state-probe-v1.json).

Input-side state has different semantics because the official UI exposes no
independent device/preamp mute buttons; the visible input-strip mute buttons
belong to the selected mixer. Recovered application paths map sentinel input
fader values into the input state bytes:

- input 1/2 `-7 dB` (`0xF900`, bytes `00 F9`) sets their state flag;
- recovered internal Input 3/4 handling maps raw zero to its state flags, but
  there is no corresponding official hardware-gain control.

The public backend exposes Windows hardware output mute, Input 1/2 preamp gain,
48V phantom power, 80 Hz high-pass, phase inversion, and all three paired
output gains as writable on the approved profile. It does not expose the
control center's host DSP mute or Input 3/4 level/state fields. All other device
SET traffic remains static-only evidence: do not enable another write without
a one-property capture and read-back test. The UI-derived Input 1/2 range is
`-7..48 dB`, and the output slider range is `-60..0 dB`; rejected values and
firmware-specific ranges outside the validated profile are untested.

A sanitized read-only observation and decoder expectation is stored in
[`fixtures/stream4x5/vendor-api-read-v1.json`](../../fixtures/stream4x5/vendor-api-read-v1.json).
It contains no serial number or device-instance path.

### Status block, 20 bytes

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 4 | Input 1 overflow counter, little-endian `u32` |
| 4 | 4 | Input 2 overflow counter, little-endian `u32` |
| 8 | 4 | Input 3 overflow counter, little-endian `u32` |
| 12 | 4 | Input 4 overflow counter, little-endian `u32` |
| 16 | 4 | Reserved/unknown; zero in the read-only observation |

The control center polls the block, compares each counter with the prior
value, and exposes clipping for inputs 1 and 2. These are counters, not audio
meters. Poll frequency and counter wrap/reset behaviour remain unverified.

### Persistent block, 64 bytes

| Offset | Size | Meaning |
|---:|---:|---|
| 0 | 9 | ASCII serial field including terminator; always redact |
| 9 | 7 | Unknown/reserved |
| 16 | 4 | Sample rate, little-endian `u32`, used by the control center only for firmware `>= 0x0200` |
| 20 | 42 | Unknown/reserved; must be preserved by read-modify-write |
| 62 | 2 | CRC-16, stored little-endian |

CRC is CRC-16/Modbus: initial value `0xFFFF`, reflected polynomial `0xA001`, no
final XOR. The CRC covers bytes `0..61`; writing it little-endian at `62..63`
makes the CRC over all 64 bytes equal zero. A 62-byte all-zero test vector has
CRC `0xAB01` and is stored as bytes `01 AB`.

The observed pre-`0x0200` device returned a block through this operation, but
its full-block CRC did not validate. That matches the control center, which
does not consume this block on old firmware. Persistent writes are therefore
unsupported on the observed firmware and unverified on all newer firmware.

## Standard UAC2 clock reads

Read-only calls against clock source `0x29` succeeded:

| Operation | Setup fields | Observed response | Evidence |
|---|---|---|---|
| Current frequency | `A1 01`, `wValue=0x0100`, `wIndex=0x2900`, length 4 | `80 BB 00 00` = 48000 Hz | S+R |
| Frequency range | `A1 02`, `wValue=0x0100`, `wIndex=0x2900`, length 64 | 14 bytes: one subrange, min=max=48000, resolution 0 | R |
| Clock validity | `A1 01`, `wValue=0x0200`, `wIndex=0x2900`, length 1 | `01` | R |
| Clock selector | `A1 01`, `wValue=0x0100`, `wIndex=0x2800`, length 1 | `01` | R |

The range response only described the active 48 kHz state, while the control
center advertises 44.1, 48, and 96 kHz from a static list. The Windows
`GetSupportedSampleRates` call independently returned only `[48000]` on the
observed device. `Get/SetSampleRate` use driver IOCTLs
`0x80882104`/`0x80882108`. A gated attempt while an ASIO client was active was
rejected with `TSTATUS_ASIO_IN_USE`; after clients were stopped, 44.1 kHz was
rejected with `TSTATUS_INVALID_SAMPLE_RATE`. A no-op set of the reported current
48 kHz value succeeded and read back as 48 kHz, confirming the setter ABI but
not a real rate change. The device remained at 48 kHz after every attempt.
Public sample-rate writes therefore remain disabled. The sanitized record is
[`vendor-api-sample-rate-write-v1.json`](../../fixtures/stream4x5/vendor-api-sample-rate-write-v1.json).
Linux should leave sample-rate negotiation to ALSA/PipeWire until a
coexistence-safe write sequence is captured.

## What is not a USB device protocol

The following controls are sent to `TUSBAUDIO_GetDspProperty` and
`TUSBAUDIO_SetDspProperty`, which become IOCTLs `0x80882200` and `0x80882204`.
The installed INF attaches `dgtstream_mixer_ducker.sys` as an upper filter and
enables its mixer plugin. Static analysis of the signed filter recovered its
PCM callbacks, property dispatchers, matrix/attenuation loops, three Ducker
instances, FX invocation, and meter accumulators. The callbacks consume arrays
of channel-buffer pointers plus a PCM sample count; they do not convert these
property blocks into Stream 4x5 class requests:

- mixer matrix weights;
- output/software-bus attenuation;
- peak/RMS meters for application, device, and virtual channels;
- Ducker;
- compressor, equalizer, and reverb effects;
- plugin performance counters.

These features are Windows driver DSP state. They cannot be implemented on
Linux by sending the same byte structures over `rusb`. Linux equivalents need
an explicit PipeWire/ALSA userspace routing and DSP design and must not capture,
proxy, or replace the hardware audio stream inside the device backend.

The complete static evidence, including the meter record and Q8.24 sample
processing, is in
[`windows-filter-driver.md`](windows-filter-driver.md). Its sanitized artifact
summary is
[`windows-filter-static-v1.json`](../../fixtures/stream4x5/windows-filter-static-v1.json).

## Linux implementation gate

Before enabling even the confirmed reads in `lewitt-backend-linux`:

1. Capture the setup packets with USBPcap or reproduce them with `rusb` on
   Arch, and promote the operation to U evidence.
2. Verify whether EP0/interface-recipient transfers can be issued without
   claiming or detaching interface 0 while `snd-usb-audio` owns the device.
3. Verify ALSA/PipeWire audio remains active during reads and one opt-in write.
4. For each write, change one property only, read back the full block, record
   failure/stall behaviour, and restore the prior value.
5. Preserve unknown bytes. Never zero-fill the persistent block or infer
   fields from UI names.
6. Keep unknown firmware read-only and keep all hardware-writing tests
   explicitly opt-in.

The DFU interface and every firmware API are permanently out of scope.
