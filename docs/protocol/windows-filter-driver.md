# Windows mixer/Ducker filter driver

This document records an offline static analysis of the installed x64
`dgtstream_mixer_ducker.sys` used with Stream 4x5. It complements
[`windows-vendor-api.md`](windows-vendor-api.md): that document describes the
user-mode DLL boundary, while this one describes what the kernel filter does
after the DSP-property IOCTL reaches the driver stack.

Nothing in this document authorizes loading the driver from the application,
issuing an unchecked IOCTL, or enabling a hardware write. No proprietary
binary, raw disassembly, serial number, device-instance path, or firmware data
is stored in this repository.

## Evidence notation and result

- **I**: installed package metadata or INF declaration.
- **S**: offline static analysis of the exact binary identified below.
- **R**: read-only observation through the registered vendor API.
- **U**: USB bus capture.

This analysis contributes I+S evidence only. The important result is:

> `dgtstream_mixer_ducker.sys` is a host-side PCM DSP upper filter. Its mixer,
> attenuation, Ducker, FX, and Peak/RMS properties are not converted into a
> private Stream 4x5 USB request.

The recovered processing callbacks receive arrays of channel-buffer pointers
and a sample count, modify or accumulate over those PCM samples, and maintain
driver-resident DSP state. The USB configuration still exposes only the UAC2
playback endpoint `0x01` and capture endpoint `0x81`; there is no separate
physical ASIO or DSP-meter endpoint.

## Analysed artifact

| Field | Value |
|---|---|
| File | `dgtstream_mixer_ducker.sys` |
| Architecture | PE32+ x86-64 |
| Size | 200392 bytes |
| SHA-256 | `5E6A2890A73C80D0F8C7B1C7524FAA45F85CA66D990F46E18F683B112D085983` |
| File version | `4.67.0.0` |
| Product version | `4.67.0.0 x64 release` |
| Authenticode | valid; Microsoft Windows Hardware Compatibility Publisher |
| Exports | none |
| Imported modules | `ntoskrnl.exe`, `HAL.dll` |

The embedded framework identification names the Thesycon USB-audio DSP
framework and version 4.67. The CodeView record names a mixer/Ducker PDB, but
its build-machine path is intentionally not retained.

A machine-readable, sanitized summary is in
[`windows-filter-static-v1.json`](../../fixtures/stream4x5/windows-filter-static-v1.json).

## Installation and stack position

The installed INF associates Stream 4x5 with `USB\VID_29C2&PID_0011`, installs
both `dgtstream.sys` and `dgtstream_mixer_ducker.sys`, and writes:

```text
UpperFilters = dgtstream_mixer_ducker
Config\EnablePlugin = 1
```

The mixer/Ducker service is a demand-start kernel driver. Its entry path uses
ordinary WDM device creation, stack attachment, remove locks, PnP, power, and
IRP forwarding. It does not import the USB driver stack or a USB request API
directly. Absence of such an import is not sufficient by itself to rule out
forwarded USB IRPs; the stronger evidence is the recovered sample-processing
code described below.

The main WDM dispatch assignments recovered from the framework initialization
are:

| IRP major function | Handler RVA |
|---|---:|
| `CREATE` | `0x77C0` |
| `CLOSE` | `0x7720` |
| `READ` | `0x7B40` |
| `WRITE` | `0x7C80` |
| `FLUSH_BUFFERS` | `0x7900` |
| `DEVICE_CONTROL` | `0x7860` |
| `INTERNAL_DEVICE_CONTROL` | `0x79A0` |
| `POWER` | `0x7AC0` |
| `SYSTEM_CONTROL` | `0x7BE0` |
| `PNP` | `0x7A40` |

These handlers are mostly framework wrappers: they acquire the remove lock,
dispatch to a per-device object, and forward or complete the IRP as required.
They are not a public ABI for this project.

## DSP plugin interface

The filter registers a private callback table with the USB-audio class driver.
Important recovered callback RVAs are:

| RVA | Static role |
|---:|---|
| `0x18F0` | configure channel counts/sample rate and construct DSP state |
| `0x1BD0` | get a DSP property block |
| `0x2190` | process one host PCM stream direction/path |
| `0x2550` | process the other host PCM stream direction/path |
| `0x2850` | set a DSP property block |

The exact framework names and the direction assigned to each processing
callback are not present in the binary, so they remain unnamed. A neighbouring
callback returns `0x00040043`, consistent with framework version 4.67. The
binary also contains explicit interface-version and interface-size mismatch
diagnostics.

Initialization constructs:

- mixer/matrix and attenuation state;
- exactly three Ducker instances, each with independent state;
- an FX engine;
- a dynamically sized meter table based on sample rate and configured channel
  counts.

The processing callbacks receive channel-buffer pointer arrays and a sample
count. Their call graph reaches the FX engine, all three Ducker instances,
matrix/attenuation loops, and meter update loops. No USB control-transfer
builder or endpoint transaction occurs on this path.

## Property-block validation

Both property entry points require at least the common eight-byte header:

```text
offset 0  u32 signature
offset 4  u32 property_id
```

Recognized signatures are ASCII `M` (`77`), `D` (`68`), and `F` (`70`). The
code checks initialization state, exact or derived block length, property ID,
instance/channel selectors, and range constraints before touching DSP state.
Malformed blocks are rejected with vendor status values. The static branches
commonly return `0xEE000031`, `0xEE000041`, `0xEE000043`, or `0xEE000047`, but
only `0xEE000041` has a previously confirmed public symbol
(`TSTATUS_INVALID_PARAMETER`). The remaining symbolic names must not be
guessed from branch context.

### Mixer properties (`M`)

| ID | Direction | Length | Static result |
|---:|---|---:|---|
| 42 | get | at least 16 | returns the tuple `2, 0` at offsets 8 and 12; field names unresolved |
| 100..106 | get | 12 | return configured channel/count values; individual names unresolved |
| 200 | get/set | `8 + N*8` | matrix entries; the public DLL uses a 16-byte one-entry form |
| 400 | get/set | 14 | per-output/software-bus Q8.24 attenuation |
| 401 | get/set | 20 | enable/disable the selected meter record; layout mirrors the meter selectors |
| 402 | get | 20 | read Peak/RMS value and processing cursor |
| 500 | get | 68 | timing/performance data; reading also resets timing accumulators |

Property 200 entries select input type/index and output type/index followed by
a signed Q8.24 coefficient. The integer mixer multiplies each 32-bit sample by
the coefficient, shifts right by 24, sums inputs, and saturates to signed
32-bit output. Its setter compares against `0x04000000` (Q8.24 value 4.0), but
the complete accepted range and user-facing policy still require a live
negative/boundary test.

Property 400 performs the same Q8.24 multiplication over an output buffer.
This is software attenuation of host PCM, not a UAC feature-unit volume write.

### Meter selector and record

Property 402 uses the packed 20-byte public layout:

```text
offset 0   u32 signature = 'M'
offset 4   u32 property_id = 402
offset 8   u8  channel_type
offset 9   u8  channel_index
offset 10  u8  meter_type
offset 11  u8  meter_position
offset 12  i32 value_q8_24
offset 16  u32 processed_sample_cursor
```

The last field was previously labelled `packet_count` from the user-mode
structure shape. Static analysis shows that processing callbacks increment it
by the number of PCM samples processed, so `processed_sample_cursor` is the
more accurate name. It is not a USB packet counter.

The selectors are:

- channel type: application `0`, device `1`, virtual `2`;
- meter type: Peak `0`, RMS `1`;
- position: input `0`, pre-fader `1`, output `2`, FX1 `3`, FX2 `4`, FX3 `5`.

Each internal meter record occupies 40 bytes:

| Offset | Size | Static meaning |
|---:|---:|---|
| `0x00` | 1 | active flag |
| `0x08` | 8 | Peak magnitude or RMS square accumulator |
| `0x10` | 4 | accumulator-overflow count |
| `0x14` | 4 | RMS window sample count |
| `0x18` | 4 | processed-sample cursor |
| `0x1C` | 4 | cursor at the last property read |
| `0x20` | 4 | cached RMS result in Q8.24 |
| `0x24` | 4 | cursor at which RMS was cached |

The remaining bytes are alignment/reserved state. The selector mapper resolves
the four selector bytes to one of these records and rejects an invalid
combination. Property 401 changes the record's active flag; property 402 reads
the record.

Recovered meter behavior:

- Peak takes the absolute magnitude of each signed 32-bit PCM sample and keeps
  the running maximum before conversion to Q8.24.
- RMS accumulates squared samples, explicitly accounts for 64-bit accumulator
  overflow, and computes an integer square root over an approximately 40 ms
  window.
- The window length is `floor((40 * sample_rate + 500) / 1000)` samples.
- If the processing cursor advances by more than `5 * sample_rate` samples
  without a property read, the record is made inactive to avoid unused meter
  work.
- Reading an inactive meter returns zero and reactivates it. An active read
  updates the last-read cursor.

The static read path does not visibly clear the Peak accumulator on each
property read. Its complete reset/decay lifecycle, wrap behavior, and behavior
across stream restart still need a live kernel-property test.

## Ducker and FX execution

The `D` property dispatcher enforces the 13-byte scalar and 12-byte channel
forms documented in [`windows-vendor-api.md`](windows-vendor-api.md). It
accepts instances `0..2`; its instance-count getter returns three. The PCM
processing path iterates exactly those three Ducker state objects. This proves
that the Ducker is performed on host samples rather than programmed into
device firmware.

The `F` dispatcher accepts property IDs `1..9`, including the 22-byte PSP
parameter form already recovered from the control-center DLL. The FX engine is
called from the PCM processing path. The binary contains compressor,
equalizer, and reverb parameter/state handling, but this pass does not assign
unproven internal routines to named audio algorithms beyond the property
mappings already established in the user-mode document.

## Consequences for ASIO, UAC, and Linux

ASIO is a Windows host API/driver path, not an extra USB endpoint on this
device. Both ASIO and Windows shared/exclusive audio ultimately use the same
physical UAC2 isochronous endpoint pair exposed by the Stream 4x5 descriptor.
The vendor stack can still provide different scheduling, buffering, and this
kernel DSP layer, so equal USB endpoints do not by themselves guarantee equal
round-trip latency between host APIs.

On Linux, ALSA/PipeWire can use the same UAC2 playback and capture endpoints
without an ASIO equivalent. The device-resident controls documented in
[`stream4x5.md`](stream4x5.md) may eventually be accessed through coexistence-
safe EP0 requests. The following Windows features cannot be reproduced by
sending their `M`/`D`/`F` blocks through `rusb`:

- application/device/virtual matrix routing;
- software-bus attenuation;
- driver Peak/RMS meters;
- Ducker and FX;
- driver performance counters.

Equivalent Linux features require a separately designed PipeWire/ALSA
userspace audio graph. That work belongs outside `lewitt-backend-linux`, whose
scope remains device control and diagnostics; the current v1 boundary also
forbids the application from capturing, proxying, or routing audio streams.

## Remaining verification gate

Before any Windows DSP property is exposed by the backend:

1. Exercise only a read property through the registered DLL and verify selector,
   size, returned value, and close/disconnect behavior.
2. Capture and sanitize a one-property read fixture. Do not store live PCM or a
   full kernel trace.
3. For a future opt-in write, change one property, read it back, restore the old
   value, and test invalid length/index behavior.
4. Verify the control center conflict path and stream restart behavior.
5. Keep unknown driver/API versions and unknown firmware read-only.

This static analysis closes the architectural question; it does not close the
runtime ABI or safe-write gate.
