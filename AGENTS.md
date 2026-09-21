# AGENTS.md

## Mission

Implement the Stream 4x5 cross-platform control panel described in `PLAN_1.md`. Treat that plan as the product source of truth. This is a greenfield Rust project targeting Windows 10/11 x64 and Arch Linux x86_64.

## Current Implementation State

- The Rust 2024 workspace and all five logical crates now exist.
- `lewitt-core` contains the domain model, typed backend errors, blocking
  `DeviceBackend`, serialized controller worker, native preset v1 model, and a
  fault-injectable mock backend.
- Continuous writes are coalesced at roughly 30 Hz, discrete writes are
  immediate, every successful write is read back, and meter delivery retains
  only the newest frame.
- The Windows backend now writes Input 1/2 preamp gain, 48V phantom power,
  80 Hz high-pass, and phase invert on the hardware-tested firmware `0x018A`
  profile through a full-block read-modify-write. Other Windows writes and all Linux writes
  remain deliberate `Unsupported` safety stubs. Do not weaken them until the
  corresponding operation is documented in `docs/protocol/stream4x5.md` and
  covered by a sanitized golden fixture.
- The Windows x64 host passes the current mock tests, formatting, strict
  Clippy, and documentation build. Ignored hardware tests changed and restored
  both Input 1/2 gains, 48V, high-pass states, and phase states with matching
  read-back. Arch Linux remains unverified.
- The tested lifecycle is wired into an Iced 0.14 daemon. Closing the window
  leaves the daemon alive, `--background` starts without a window, and a
  `tray-icon` menu provides Show, Reconnect, Status, and Quit. Linux selects
  the KSNI backend without GTK/AppIndicator defaults. A left click shows the
  window; showing an existing minimized window restores it, and showing an
  existing window raises it once without leaving it always on top.
- Per-user Windows named-pipe single-instance transport is wired into the
  daemon and covered by an integration test. The Linux `XDG_RUNTIME_DIR`
  Unix-socket, KSNI, X11, and Wayland configuration cross-compiles and passes
  strict Clippy for `x86_64-unknown-linux-gnu`, but has not run on Arch yet.
- Explicit autostart consent, bounded single-instance command framing, and
  English/Chinese key parity are covered by tests. Startup-entry writers are
  intentionally deferred until the remaining device work is complete.
- The Windows backend validates and loads the registered 64-bit vendor API,
  reads the hardware snapshot, and writes Input 1/2 gain, 48V, 80 Hz high-pass,
  and phase invert only on firmware `0x018A`. `lewittctl diagnose [--json]` remains
  read-only and all unverified CLI operations fail closed.

## Immediate Priorities

1. Validate and expose remaining Windows hardware controls one property at a
   time, with read-back, restoration, and a sanitized fixture for each.
2. Validate the Linux Unix-socket transport and KSNI tray in an Arch
   environment.
3. Add Linux VID/PID-only discovery and permission diagnostics in an Arch
   environment without detaching an audio interface.
4. Implement platform autostart writers last, behind the tested
   explicit-consent state machine.

## Product Boundaries

- Support only Lewitt Stream 4x5, USB VID `0x29C2` and PID `0x0011`, in v1.
- The application is a control panel. Do not implement, proxy, capture, or route audio streams.
- Windows audio remains owned by ASIO/WASAPI. Linux audio remains owned by ALSA/PipeWire.
- Do not implement firmware upload, download, recovery, or DFU operations.
- Do not add support for DGT 260, DGT 450, DGT 650, or unverified firmware by guessing.
- Do not add telemetry, automatic updates, cloud services, accounts, or network-dependent runtime features.
- Source builds are the only required distribution format in v1.

## Repository Shape

Use a Rust 2024 Cargo workspace with these logical components:

- `lewitt-core`: public domain types, backend trait, controller worker, preset model, and mock backend.
- `lewitt-backend-windows`: dynamic wrapper around the installed vendor API.
- `lewitt-backend-linux`: `rusb` implementation using the system `libusb`.
- `lewitt-app`: Iced GUI, tray lifecycle, localization, autostart, and single-instance IPC.
- `lewittctl`: scriptable CLI built on the same core and platform backends.

Keep platform-specific dependencies behind target-specific Cargo sections. The shared core must build and test without hardware or proprietary binaries.

## Core Contract

- Expose a blocking `DeviceBackend` with `enumerate`, `open`, `close`, `capabilities`, `read_snapshot`, `set_control`, `read_back`, `read_meters`, and `poll_events` behavior.
- Keep all backend calls on one dedicated worker thread. Hardware handles and vendor callbacks must not leak into GUI or CLI code.
- Use bounded channels. Coalesce continuous control writes to about 30 Hz and discard stale meter frames instead of building queues.
- Discrete controls write immediately. Every successful write must be read back. On failure, refresh from hardware and report the real state.
- Represent failures with typed errors: driver missing, permission denied, busy, disconnected, protocol mismatch, unsupported firmware, and invalid value.
- Unknown firmware is read-only until a hardware-backed compatibility test explicitly approves it.

## Platform Rules

### Windows

- Locate the 64-bit vendor API through CLSID `{ADACFE1D-A8E1-4606-9093-3A7418223B78}` and its registered `InprocServer32` path.
- Never search the current working directory for `dgtstreamapi_x64.dll` and never redistribute it.
- Validate the vendor API and driver version before enabling writes.
- Isolate all FFI and `unsafe` code in the Windows backend. Document every ABI, pointer, buffer-length, ownership, and callback lifetime invariant.
- If Lewitt Control Center is running, warn but do not terminate or modify that process. On a conflict, stop writes and refresh state.

### Linux

- Use `rusb` with the system `libusb`; do not enable a vendored static libusb build by default.
- Do not detach, replace, or rebind `snd-usb-audio` interfaces.
- Only send protocol operations proven by sanitized captures or documented golden fixtures.
- Provide an example udev rule and clear permission diagnostics. Never suggest running the GUI as root as the normal solution.
- Verify that controls and meters operate while ALSA/PipeWire audio remains active.

## Protocol Work

- Record confirmed behavior in `docs/protocol/stream4x5.md` before exposing it as a public control.
- Reverse one property at a time and record request type, request, value, index, length, payload, response, byte order, CRC, valid range, and failure behavior.
- Golden fixtures must be minimal and sanitized. Remove serial numbers, user paths, unrelated traffic, and firmware data.
- Never commit Lewitt executables, DLLs, SYS files, firmware images, installer contents, or full raw captures containing private identifiers.
- If a property ID, structure, range, or ordering rule is not verified, return `Unsupported` or keep the UI disabled. Do not infer writable protocol values from names alone.

## UI and Runtime Behavior

- Use Iced 0.14 and its daemon-style lifecycle. Use `tray-icon`; select the KSNI backend on Arch Linux.
- Closing the window hides it. The tray menu must provide Show, Reconnect, Status, and Quit. Quit must stop workers and release hardware handles.
- Autostart is opt-in during first-run onboarding. The checkbox may be preselected, but write the OS startup entry only after explicit confirmation.
- Autostart launches with `--background`. A manual second launch must signal the existing instance to show its window.
- Use a Windows named pipe and a socket under `XDG_RUNTIME_DIR` on Linux for single-instance IPC.
- Provide Chinese and English resources with identical key sets. Default to the system locale and allow manual switching.
- Poll the hardware snapshot and meters at about 60 Hz while visible so physical
  knob changes track promptly. While hidden, poll meters at 2 Hz and the full
  snapshot at 0.2 Hz. A disconnected device must not cause a crash or busy loop.
- Use an independent visual design. Do not copy vendor artwork, icons, layout assets, or branding beyond compatibility text.

## Presets and CLI

- The native preset format is versioned JSON with `schema_version: 1`.
- Presets may contain hardware controls, mixer routing, and ducker settings. Exclude serial numbers, live meters, firmware, ASIO buffer size, clock source, application settings, and autostart state.
- Import and export Lewitt XML 1.6 only for fields confirmed against the embedded schema and real exported samples. Preserve unknown XML nodes and attributes but never execute them.
- Validate an entire preset before writing. Apply confirmed operations sequentially, stop on the first failure, and refresh the full snapshot.
- The CLI must provide `list`, `status`, `watch-meters`, `get`, `set`, `preset import`, `preset export`, `preset apply`, `diagnose`, and `--json`.
- Do not expose arbitrary raw USB or unchecked vendor API writes in release builds.

## Safety and Privacy

- Hardware-writing tests must be explicitly marked and opt-in. Default tests must use mocks or read-only fixtures.
- Never change drivers, device bindings, registry driver registration, audio defaults, or security settings from the application.
- Redact serial numbers and protocol payloads in normal logs. Diagnostics stay local and are never uploaded automatically.
- Do not store secrets. Do not include proprietary binaries or confidential captures in Git history.
- Keep the project disclaimer visible: this is an unofficial compatibility project and is not affiliated with Lewitt GmbH.

## Code Quality

- Prefer safe Rust. Keep `unsafe` narrowly scoped to audited FFI boundaries with `SAFETY` comments.
- Avoid panics in device, protocol, IPC, preset, and filesystem paths. Return contextual typed errors.
- Preserve backend/platform boundaries; do not hide platform branches throughout shared business logic.
- Add unit tests for control ranges, dB conversion, protocol encoding, CRC, preset migration, translation key parity, and error mapping.
- Add mock integration tests for hotplug, coalescing, failed readback, disconnect during I/O, reconnect, tray lifecycle, and single-instance IPC.
- Hardware behavior is not complete until it is verified on both Windows and Arch Linux with the same Stream 4x5.

## Required Verification

Run these before handing off a completed change when the relevant workspace exists:

```text
cargo fmt --all -- --check
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo doc --workspace --no-deps
git diff --check
```

Also build the Windows x64 target and an Arch Linux x86_64 environment for platform changes. Report hardware-dependent checks separately from mock, compile, and static checks. A successful build alone is not evidence that USB control behavior is correct.
