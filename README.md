# pi-agent (v3)

`pi-agent` is SynthVCopilot's Rust-based, in-process Agent runtime. It builds a
C-ABI DLL (`pi_agent.dll`) that [`pi-desktop`](https://github.com/SynthVCopilot/pi-desktop)
loads through P/Invoke; it does not start a sidecar process for the desktop-to-agent
connection.

## What exists today

- **Anthropic provider**: configured creation is available through
  `pi_agent_create_json`; the unconfigured `pi_agent_create` remains an explicit
  echo-provider fallback for smoke testing.
- **SynthV bridge**: the FFI can launch and call the host-neutral
  [`synthv-agent-bridge`](https://github.com/SynthVCopilot/synthv-agent-bridge)
  MCP stdio Runtime.
- **pi-audio**: local Python audio probing and vocal/instrumental pair-diff to
  monophonic MIDI. Optional Basic Pitch and PANNs capabilities are detected by
  that component.
- **CVRS**: a cross-version `.svp` reference-audio helper. It reads and clones
  the target structure to a new output file; it does not render audio or
  overwrite the source project.
- **Component catalog**: static metadata for the wider component family is
  exposed to the desktop shell. FFmpeg has runtime status and lifecycle jobs;
  the other catalogued components do not yet share that lifecycle backend.

The repository does not yet include implementations for the catalogued ML
components such as local Whisper, source separation, instrument/genre analysis,
beat detection, or full Sound-to-MIDI.

## FFmpeg component

The managed FFmpeg backend is implemented. It provides safe, fixed-purpose
audio preparation and loudness operations, plus user-triggered install, update,
and uninstall lifecycle jobs. It does not change the global `PATH`, execute
caller-supplied FFmpeg arguments, or accept remote input URLs.

See [the FFmpeg component contract](docs/ffmpeg-component.md) for the C-ABI,
job JSON, release manifest, safety rules, licensing, and the hand-off contract
for `pi-desktop`.

## Workspace

```
pi-agent/
├─ crates/pi-agent-core/  # agent loop, conversation history, component metadata
├─ crates/pi-agent-mcp/   # MCP stdio client and SynthV bridge
├─ crates/pi-agent-provider/ # Anthropic and component configuration
├─ crates/pi-agent-components/ # managed FFmpeg and safe audio execution
├─ crates/pi-agent-ffi/   # cdylib: pi_agent.dll C ABI for WinUI 3
└─ components/            # pi-audio and CVRS Python component implementations
```

All component data, outputs, configuration, and history use the per-user root
`~/.SynthVcopilot` (on Windows, `%USERPROFILE%\\.SynthVcopilot`).

## Build and checks

Rust 1.92+ with the MSVC toolchain is required for the Windows DLL.

```powershell
cargo build
cargo build -p pi-agent-ffi --release
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The Windows workflow also checks the `aarch64-pc-windows-msvc` target.

## C ABI

The public declarations are maintained in
[`include/pi_agent.h`](include/pi_agent.h). Every non-NULL `char*` returned by
this library must be released exactly once with `pi_string_free`; opaque handles
must be released with their matching destroy function.

## License

This repository is Apache-2.0; see [LICENSE](LICENSE). The managed FFmpeg
binary is a separately licensed LGPL build with its own source and license
notices, documented in the FFmpeg component contract.
