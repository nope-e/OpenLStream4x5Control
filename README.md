# lewitt-ctl

An experimental, cross-platform control panel for the Lewitt Stream 4x5.

> This is an unofficial compatibility project and is not affiliated with or
> endorsed by Lewitt GmbH. Product names are used only to describe
> compatibility.

The project is at an early implementation stage. The Windows app can load the
registered vendor API and display a verified read-only hardware snapshot. Real
device writes stay disabled until each operation has a documented, sanitized
hardware fixture and an opt-in hardware test.

## Scope

- Stream 4x5 only (`VID 0x29C2`, `PID 0x0011`).
- Hardware controls, routing, presets, and meters only; audio remains managed by
  ASIO/WASAPI or ALSA/PipeWire.
- The v1 UI omits Ducker, compressor, EQ, and reverb. Linux does not reproduce
  the Windows virtual-channel mixer; its meter design is limited to passive
  physical-input/output RMS observation outside the USB backend.
- Source builds for Windows 10/11 x64 and Arch Linux x86_64.
- No firmware operations, telemetry, cloud services, or automatic updates.

See [`PLAN_1.md`](PLAN_1.md) for the product plan,
[`docs/protocol/stream4x5.md`](docs/protocol/stream4x5.md) for Linux USB
evidence, and
[`docs/protocol/windows-vendor-api.md`](docs/protocol/windows-vendor-api.md)
for the recovered Windows vendor API boundary. The host-side mixer, Ducker,
FX, and meter implementation is documented separately in
[`docs/protocol/windows-filter-driver.md`](docs/protocol/windows-filter-driver.md).

Linux users will eventually need ordinary-user USB control permission. An
example rule is provided at
[`packaging/99-lewitt-stream4x5.rules`](packaging/99-lewitt-stream4x5.rules);
running the GUI as root is not a supported setup.
