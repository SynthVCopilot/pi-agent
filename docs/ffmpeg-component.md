# FFmpeg component contract

> Status: implemented public component contract.

## Scope and safety boundary

The FFmpeg component is an auxiliary **audio** service owned by `pi-agent`.
It offers four processing operations:

1. `probe` — inspect an existing local media file.
2. `prepare` — create WAV audio with optional trim, resampling, channel count,
   and PCM sample-format conversion.
3. `loudness_analyze` — measure integrated loudness, true peak, LRA, and
   threshold.
4. `loudness_normalize` — perform two-pass EBU R128 normalization with
   caller-supplied loudness, true-peak, and LRA targets.

Normalization operates on `0:a:0`, preserves that stream's sample rate and
channel layout, and writes a PCM s24 WAV. Both passes select the same stream.

The component accepts existing **absolute local file paths** only. It rejects
URLs, device paths, pipes, stdin, relative paths, output paths outside its data
root, arbitrary filter graphs, and arbitrary FFmpeg/ffprobe arguments. Jobs
write new files below `~/.SynthVcopilot/output/ffmpeg`; they never overwrite a
caller-selected source path.

An Agent may expose and call only the four processing operations after the
component is ready. Install, update, and uninstall are human actions initiated
by the desktop UI; the Agent must never initiate them.

## Resolution and storage

The resolver chooses exactly one healthy binary pair in this order:

1. **explicit** — a user-selected FFmpeg directory stored in component
   configuration and validated by `ffmpeg -version` and `ffprobe -version`.
2. **managed** — the currently active, checksum-verified private installation.
3. **system** — `ffmpeg` and `ffprobe` discovered through the inherited process
   PATH and validated by both version commands.

It reports the selected `source` as `explicit`, `managed`, `system`, or
`unavailable`.
No operation modifies global or user `PATH`, registers a shell association, or
writes outside the component data root.

```
~/.SynthVcopilot/
├─ components/ffmpeg/<release>-<x64|arm64>/bin/{ffmpeg,ffprobe}.exe
├─ components/ffmpeg/current.json       # active managed release pointer
├─ components/ffmpeg/.component.lock    # cross-process operation lock
├─ components/ffmpeg/license/           # supplied notice and source link
├─ downloads/ffmpeg/                    # temporary archive only
└─ output/ffmpeg/                       # job outputs only
```

On Windows `~` means `%USERPROFILE%`. Managed installation extracts to a
temporary sibling, validates both executables, then atomically updates
`current.json`; a failed update leaves the prior healthy installation active.
Uninstall removes only the matching managed component directory and never
removes explicit or system installations.

The lock file is held with Windows deny-sharing semantics. It serializes
processing and lifecycle work across multiple Desktop/FFI host processes, while
the in-process read/write gate provides cancellable waiting and status checks.

## Fixed managed-release manifest

The first manifest is intentionally immutable, uses BtbN's LGPL non-shared ZIP
assets, and supports only Windows x64 and ARM64. The implementation must embed
the values below or load byte-identical values from a versioned, signed project
manifest; it must not follow a `latest` release URL.

| Architecture | Release / asset | SHA-256 |
|---|---|---|
| `x86_64` | [`autobuild-2026-08-04-21-26` `ffmpeg-n8.1.2-34-g9b6c8969e0-win64-lgpl-8.1.zip`](https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-04-21-26/ffmpeg-n8.1.2-34-g9b6c8969e0-win64-lgpl-8.1.zip) | `534b3bfe48de5d3d181430294602a5977a198a47a28e48a1599545eab0ab7a60` |
| `aarch64` | [`autobuild-2026-08-04-21-26` `ffmpeg-n8.1.2-34-g9b6c8969e0-winarm64-lgpl-8.1.zip`](https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-04-21-26/ffmpeg-n8.1.2-34-g9b6c8969e0-winarm64-lgpl-8.1.zip) | `69ae299f3e8a0795e4fd3def0678e7ac71dbfec6795199d425b04608c71179c4` |

The asset table uses upstream architecture names; managed directory suffixes
are `x64` and `arm64` as shown in the storage layout.

The archived upstream checksum file is
[checksums.sha256](https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-04-21-26/checksums.sha256).
Downloads must be streamed to a temporary file, checked before extraction, and
must reject ZIP entries with absolute paths or `..` traversal.

## C ABI and job JSON

All text parameters are UTF-8, NUL-terminated C strings. All `char*` results
are allocated by `pi_agent.dll` and must be freed once with `pi_string_free`.
`PiJob*` is opaque, must not be shared concurrently, and must be freed with
`pi_job_destroy`, including after a failed or cancelled job.

```c
char*  pi_components_status_json(void);
PiJob* pi_component_action_start(const char* component_id, const char* action);
PiJob* pi_ffmpeg_job_start(const char* request_json);
char*  pi_job_status_json(PiJob* job);
void   pi_job_cancel(PiJob* job);
void   pi_job_destroy(PiJob* job);
```

`pi_component_action_start` accepts only `component_id == "ffmpeg"` and the
actions `install`, `update`, or `uninstall`. Invalid UTF-8, unknown identifiers
or actions, and an already-running lifecycle job return `NULL`. Unsupported
architectures return a non-NULL job that terminates with a structured
`invalid_state` error. A `NULL` startup result has no separate error object.

`pi_components_status_json` returns an array of `ComponentView` values, each
with a static `spec` and current `status`. It is not a keyed object. The FFmpeg
entry is structurally equivalent to:

```json
[
  {
    "spec": {
      "id": "ffmpeg",
      "kind": "ffmpeg",
      "display_name": "FFmpeg",
      "description": "音视频转码与抽取；分离/识别/Sound→MIDI 的前处理基础。",
      "version": "latest",
      "audience": "both",
      "download_url": ""
    },
    "status": {
      "id": "ffmpeg",
      "state": "not-installed",
      "source": "unavailable",
      "available_version": "n8.1.2-34-g9b6c8969e0-lgpl-8.1",
      "can_install": true,
      "can_update": false,
      "can_uninstall": false,
      "error": "..."
    }
  }
]
```

`status.source` is one of `explicit`, `managed`, `system`, or `unavailable`.
`status` also includes `installed_version` and `executable_dir` when a healthy
binary pair is resolved.

Component configuration is process-wide. Before an Agent is created, these
APIs read `~/.SynthVcopilot/config.json`. A successful
`pi_agent_create_json` call makes its `ffmpeg` object the process-wide override,
so Agent tools, status calls, and direct jobs resolve the same executable pair.
The configuration shape is:

```json
{"ffmpeg":{"preference":"auto","system_bin_dir":"C:\\Tools\\ffmpeg\\bin"}}
```

`preference` is `auto`, `managed`, or `system`; omit `system_bin_dir` to use
managed-then-PATH resolution in `auto` mode.

`pi_ffmpeg_job_start` recognizes these request shapes:

```json
{"operation":"probe","input":"C:\\audio\\take.wav"}
{"operation":"prepare","input":"C:\\audio\\take.flac","output_name":"take.wav","start_seconds":0,"duration_seconds":12.5,"sample_rate":44100,"channels":1,"sample_format":"s24"}
{"operation":"loudness_analyze","input":"C:\\audio\\take.wav"}
{"operation":"loudness_normalize","input":"C:\\audio\\take.wav","output_name":"take-normalized.wav","target_lufs":-16,"max_true_peak_db":-1.5,"target_lra":11}
```

## Agent permission boundary

The Agent-visible tool surface is deliberately read-only: it exposes only
`ffmpeg_probe` and `ffmpeg_loudness_analyze`. `prepare` and
`loudness_normalize` remain available through `pi_ffmpeg_job_start` for
`pi-desktop`, which must show the operation to the user and obtain their
confirmation before it starts a job that writes a WAV file.

`pi_job_status_json` returns a stable envelope. `progress` ranges from `0` to
`1`; it is `1` on success. `phase` reports the active lifecycle or processing
phase, and `error` is a structured object with `code`, `message`, and optional
`details`.

```json
{
  "id": "7",
  "state": "running",
  "phase": "processing",
  "progress": 0.42
}
```

Optional fields are omitted rather than serialized as `null`: a successful
terminal job adds `result`, while a failed job adds `error`.

Valid `state` values are `queued`, `running`, `succeeded`, `failed`, and
`cancelled`. Current phases include `queued`, `running`, `waiting`, `resolving`, `downloading`,
`verifying`, `extracting`, `checking`, `uninstalling`, `processing`, `complete`,
`cancelled`, and `failed`. Failed jobs return a stable snake_case machine code,
such as `invalid_input`, `not_found`, `download_failed`, `integrity_mismatch`,
`unsafe_archive`, `process_failed`, or `cancelled`.

## Licensing and provenance

The component distributes the two pinned **LGPL** BtbN builds, not a locally
discovered GPL build. It retains the archive's license notice plus the release,
asset, checksum, and build-version provenance under the managed license
directory. The pinned archive identifies the FFmpeg source snapshot as
[`9b6c8969e0`](https://github.com/FFmpeg/FFmpeg/tree/9b6c8969e0). Licence
details and downstream obligations are available from
[FFmpeg legal](https://ffmpeg.org/legal.html),
[FFmpeg downloads](https://ffmpeg.org/download.html), and
[BtbN FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds).

## `pi-desktop` hand-off

The desktop component page owns the user interaction:

- Read `pi_components_status_json` at page load and refresh it after a job
  reaches a terminal state; poll only `pi_job_status_json` while it runs.
- Start only user-confirmed lifecycle jobs; render `phase`, `progress`, and
  structured errors; provide cancel for queued/running jobs.
- Never construct FFmpeg command lines or download URLs in C#.
- Send audio requests only through the four JSON operation schemas and display
  the returned result rather than inferring success from process output.

The FFmpeg backend owns resolution, download, verification, extraction, command
construction, cancellation, and output containment. This keeps the UI generic
for future components while preventing it from becoming a second installer or
process runner.
