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
| U | Confirmed as the actual USB setup packet by USBPcap or direct Linux `libusb`/`rusb` observation. |

The findings below have S and, where stated, R evidence. There is no U evidence
yet. In particular, the vendor API can transform a request before it reaches
the USB bus, so a successful Windows API call is not by itself a Linux-ready
golden capture.

## Evidence snapshot

- Analysis date: 2026-09-20.
- Vendor API version returned by `TUSBAUDIO_GetApiVersion`: `5.2`
  (`0x00050002`).
- Installed API/driver file version: `4.67.0.0`.
- Connected test firmware reported by `GetDeviceProperties`: `0x018A`.
- Device enumeration, open, close, configuration-descriptor read, current
  sample-rate read, clock reads, and the three private read operations below
  succeeded.
- No control, sample-rate, mixer, DSP, firmware, or DFU write was performed.
- Serial number and Windows device-instance ID were intentionally discarded.

Firmware `0x018A` is an observation, not an approved writable firmware. It is
below the control center's `0x0200` behaviour boundary and must remain read-only
until write captures are available.

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
| Settings | GET / SET | `0x0000` | `0x3300` | 40 | S+R for GET; S only for SET |
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
| 12 | 6 | Unmapped output state flags 1..6 | Recovered serializer emits `0` or `1`, but a live comparison disproved their mapping to the visible output-mute buttons. Do not expose them. |
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

The official UI exposes output mute as an independent button, but those buttons
are **not** bytes `12..17` in this device block. In a simultaneous read-only
observation, the official UI showed output pairs 1/2 and 5/6 muted and 3/4
unmuted while all six bytes at `12..17` were zero. Neither direct nor inverted
boolean decoding can represent that state, so the public backend leaves these
six bytes unmapped.

Static IL analysis identifies the visible physical-output button path as
`Click_buttonMute -> Mixer8x6.UpdateAttenuation -> UpdateAttenuationHW ->
SetOutAttenuationSafe`. The three physical pairs use preset fields `out2Mute`,
`out3Mute`, and `out4Mute`; a muted pair is sent as exact zero attenuation for
both channels through DSP property 400. A live `TUSBAUDIO_GetDspProperty` read
for channel type `DEVICE` (`1`), indices `0..5`, returned Q8.24 values
`[0, 0, 16720723, 16720723, 0, 0]`, matching the visible on/off/on state. The
Windows backend therefore reads `OutputMute` from this DSP property and keeps
the independent device output-gain values unchanged.

This mute is Windows host-filter state for a physical output path, not a
device-resident USB mute. It cannot be implemented on Linux by copying the
property block to EP0. The Linux backend must omit `OutputMute` from its
capabilities, and the shared GUI hides the control instead of synthesizing an
off state. A future PipeWire implementation would be a separate optional
host-audio provider, not part of `lewitt-backend-linux`. No output-mute write
is enabled yet.

The six device-block flags still require a separate hardware-mute experiment.
With Windows DSP mute disabled, use the official output fader to move exactly
one pair to `-60 dB`, compare bytes `12..17` and a USBPcap trace before/after,
then close the control center and verify whether the physical output remains
muted across a reconnect. Restore the original gain after the test. Until that
sequence is captured, the application must neither write nor expose these
flags, and must not describe them as hardware mute.

Input-side state has different semantics because the official UI exposes no
independent device/preamp mute buttons; the visible input-strip mute buttons
belong to the selected mixer. Recovered application paths map sentinel input
fader values into the input state bytes:

- input 1/2 `-7 dB` (`0xF900`, bytes `00 F9`) sets their state flag;
- recovered internal Input 3/4 handling maps raw zero to its state flags, but
  there is no corresponding official hardware-gain control.

The public read-only backend exposes Windows DSP output mute state, Input 1/2
preamp gain and their derived endpoint state, but does not expose Input 3/4
level/state fields or the six unmapped output flags. Device SET traffic is
still static-only evidence: do not enable any write without a one-property
capture and read-back test. The UI-derived Input 1/2 range is `-7..48 dB`, and
the output slider range is `-60..0 dB`; rejected values and firmware-specific
ranges are untested.

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
center advertises 44.1, 48, and 96 kHz from a static list. It is not proof that
a direct `SET_CUR` is safe. The Windows `Get/SetSampleRate` exports use driver
IOCTLs `0x80882104`/`0x80882108`; no sample-rate write or raw bus capture was
performed. Linux should leave sample-rate negotiation to ALSA/PipeWire until a
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
