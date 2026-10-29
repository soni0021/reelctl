# Spec: Smelter as an optional render backend (`smelter-backend`, P2 slice)

Status: written 2026-10-29 under the user's "p1-p3"; the user has not reviewed it line by line. The open questions at the end carry the recommended answers. Branch: `feature/smelter-backend`, cut from `main` (b312ffd). Follows [planer.md](../../planer.md) ("replace [headless-browser rendering] with native Rust wgpu rendering via software-mansion/smelter") and the P2 row. Builds on [timeline-editor](timeline-editor.md) and [ADR-019](../decisions/ADR-019-edit-an-opentimelineio-project-with-our-own-subset-and-render-with-ffmpeg.md). The decision record is [ADR-023](../decisions/ADR-023-drive-smelter-as-a-separate-process-over-its-http-api.md). The plan is [smelter-backend-plan](smelter-backend-plan.md).

## Objective

`reelctl render` turns an OpenTimelineIO project into an mp4 with one ffmpeg filter graph. planer.md names Smelter, a GPU (wgpu) compositor, as the renderer of the editing engine. This slice adds Smelter as a second, optional render backend behind a trait, and keeps ffmpeg the default:

```
reelctl render --project edit.otio --out out.mp4                       # ffmpeg, as before
reelctl render --project edit.otio --out out.mp4 --backend smelter [--smelter /path/to/smelter]
```

The project, every edit command and the ffmpeg render do not change. The Smelter backend reads the same `.otio` subset and gives the same timeline: the same frame count, the same cut points, the same punch-in, the same card. Where it cannot give the same result (below, "What Smelter cannot reproduce"), it says so on the command line.

Not in this slice: manimgx overlays, RobustVideoMatting cutouts, live (real-time) rendering, Smelter's web renderer, shaders, transitions between clips, the Smelter TypeScript SDK, and the overlay track (it does not exist in the project format on `main`; when another slice adds one, the backend refuses a project that has it until it learns it).

## Step 0: what Smelter is (checked 2026-10-29)

Sources: github.com/software-mansion/smelter (README, LICENSE, releases through the GitHub API), smelter.dev/docs, smelter.dev/http-api/*, and the `smelter-api` crate source on `master` (the request types). Every field below that is used by the backend was read in one of those and then exercised against a real server (see Evidence).

- **What:** "Toolkit for real-time, low latency, programmable video and audio mixing" by Software Mansion. Formerly `live-compositor` (the repository and its releases carry the new name; the rename is not described in the pages read, so the history is taken from the repository, not documented).
- **How it runs:** a standalone server binary (`smelter`, a shell wrapper that runs `dependency_check` then `exec`s `smelter_main`), a Docker image (`ghcr.io/software-mansion/smelter`), or built from source. The control surface is an HTTP API (port `SMELTER_API_PORT`, default 8081) plus a WebSocket at `/ws` that sends events. There are a TypeScript SDK (`OfflineSmelter`) and a Membrane plugin on top of the same server. It is not published as a usable Rust library crate for embedding (the workspace crates `smelter-core`, `smelter-render`, `smelter-api` are internal).
- **Releases:** v0.6.0 on 2026-06-09 (v0.5.0 2025-12-02, v0.4.2 2025-04-29). Assets include `smelter_darwin_aarch64.tar.gz` (18.9 MB; sha256 `dbd0500c...a933a9`, matched the digest GitHub publishes), `smelter_darwin_x86_64`, `smelter_linux_aarch64`, `smelter_linux_x86_64`, and web-renderer variants (137 MB and up).
- **Platform:** macOS arm64 runs. On this machine (Apple M3, macOS 26.5) the server found "Apple M3 ... backend: Metal" and rendered. The binary links Homebrew's FFmpeg dynamically (`@rpath/libavcodec.62.dylib`, rpaths `/opt/homebrew/opt/ffmpeg/lib`, `/opt/homebrew/opt/ffmpeg@8/lib`): with FFmpeg 8.1 installed it starts; a different FFmpeg major version would not load.
- **Licence: custom, not MIT and not an OSI licence.** GitHub reports `NOASSERTION`. The `LICENSE` file says offline processing (non-real-time, batch, pre-recorded content) is under terms "analogous to the permissive MIT license", and that real-time processing (live streaming) is limited to organisations with 50 or fewer people and under 10,000 peak concurrent viewers, with a 90-day trial and a commercial licence from $1,000 per month; it prohibits offering a SaaS that provides programmatic video control using the software, and bundling it into a distributed product, without a commercial licence. It also lets the licensor name the user in marketing. This slice uses offline mode only and never bundles or redistributes the binary, but a product that offered `reelctl` rendering as a service would have to read that file. This is why the binary is a separately installed program and is never vendored.
- **Offline mode:** `SMELTER_OFFLINE_PROCESSING_ENABLE=true` turns on ahead-of-time processing and never-drop output frames (documented; the integration test `offline_processing.rs` registers an mp4 input and an mp4 output, schedules the unregisters, sends `start` and waits for the WebSocket event `OUTPUT_DONE`). The server renders faster than real time (a 3 s timeline took 0.4 s).
- **Input:** `POST /api/input/:id/register` with `{"type":"mp4","path"|"url", "seek_ms", "offset_ms", "required", "loop", "decoder_map", ...}`: H.264 video and AAC audio only, first video and first audio track only. The response is `{"video_duration_ms", "audio_duration_ms"}`.
- **Output:** `POST /api/output/:id/register` with `{"type":"mp4","path","video":{"resolution":{"width","height"},"encoder":{"type":"ffmpeg_h264", "ffmpeg_options":{...}},"initial":{"root":<component>},"send_eos_when":{...}},"audio":{"encoder":{"type":"aac","sample_rate"},"channels":"stereo","initial":{"inputs":[{"input_id","volume"}]},"send_eos_when":{...}},"ffmpeg_options":{...}}`.
- **Scene:** components `input_stream`, `view`, `rescaler`, `image`, `text`, `tiles`, `shader`, `web_view`, tagged by `"type"`; unknown fields are rejected. `POST /api/output/:id/update` takes `{"video":{"root"},"audio":{"inputs"},"schedule_time_ms"}`.
- **Routes used:** `GET /status`, `POST /api/image/:id/register`, `/api/input/:id/register|unregister`, `/api/output/:id/register|update`, `POST /api/start`, `GET /ws`.

## Evidence (manual experiments against Smelter 0.6.0, 2026-10-29)

These decide the translation; the numbers are in [status](../status.md#smelter-backend-2026-10-29).

1. **A seek is frame-exact.** `seek_ms` into `textured_cuts12` (24 fps, keyframe every 25, B-frames) at frame 37 (not a keyframe) gave the picture of frame 37, checked against the burned-in frame counter.
2. **An `offset_ms` in the future places the first frame.** A second input registered with `offset_ms` of 24 frames shows its first frame at output frame 24, when the scene is swapped at the right time.
3. **A scheduled scene update applies one frame late when scheduled at the frame's own time**: frames with a timestamp equal to `schedule_time_ms` still show the old scene. Scheduling half a frame early gives the exact frame (checked for scene swaps, a black gap and an image).
4. **The end of the output is exact only when the last input ends it.** `send_eos_when: {"all_inputs": true}` with every input unregistered at its end gave the exact frame count; unregistering the output on a schedule gave a frame count that moves non-monotonically with the scheduled time (69 to 75 frames for a 72-frame timeline). A timeline that ends in a card or a gap has no input at its end, so the backend registers one extra "clock" input (the first media file, `loop: true`) that is unregistered at the timeline's end.
5. **A scheduled update that is wrong fails silently to the caller.** It is logged by the server at `ERROR` ("Audio and video specification for output ... doesn't match one provided in register output request") and the update does not happen. An update must carry `video` and `audio` whenever the output was registered with both. The backend therefore reads the server's JSON log and fails when it finds an `ERROR` line.
6. **Unsupported media is not an error.** An mp4 with PCM audio registers with `audio_duration_ms: null`: the audio is dropped without a message. An HEVC mp4 registers with HTTP 500 "No suitable track in the mp4 file". A missing path is 400 `INVALID_MP4_SOURCE`; a relative path is resolved against the server's directory. The backend uses `audio_duration_ms` from the register response to refuse (exit 2) a project with audio whose media Smelter cannot hear.
7. **An existing output file is renamed, not overwritten.** So the backend writes to a fresh temporary name beside `--out` and renames, as the ffmpeg path does.
8. **Colour:** with an untagged H.264 source Smelter's picture differs from the ffmpeg render by a mean luma difference of about 15 (it reads untagged YUV as BT.709 and writes BT.709 tags); with a source tagged BT.709 the difference falls to the noise of two encodes. ffmpeg's render keeps the source's YUV values and tags. The note and the numbers are in status.
9. **The server listens on all interfaces** (`*:port`), and the documented environment has no bind-address variable. A render exposes the API (which can write a file at any path the process can reach) on the machine's network for its duration. The backend picks a free port, never reuses a server, and kills it when done; this is stated in the docs.
10. **The server exits when its parent process exits** ("Compositor parent process was terminated"), so a crashed `reelctl` does not leave one behind.

## Decision

Option (a) of three considered (full reasoning in ADR-023): drive a separately installed Smelter process over its documented HTTP API. Not (b) a Cargo dependency (the engine is internal workspace crates, not a published embeddable library; it would need a GPU stack and FFmpeg 8 development libraries in every build and its licence is not permissive for redistribution) and not (c) spec only (the binary runs here, so the translation is verified, not guessed).

- `reelctl` never downloads, installs or vendors Smelter. The user installs it (`brew`-style or the release tarball) and points `--smelter` / `SMELTER_BIN` at `smelter` or `smelter_main`.
- No new Cargo dependency. The HTTP and WebSocket client is about 200 lines over `std::net` (loopback, JSON, `Connection: close`).

## Contract

### The trait

In `crates/reelctl/src/render_backend.rs`:

```rust
pub trait RenderBackend {
    fn name(&self) -> &'static str;
    fn render(&self, job: &RenderJob<'_>) -> Result<RenderOutcome>;
}
```

`RenderJob` is the project (path and parsed), `--out`, and `--size`. `RenderOutcome` is frames, duration, size, the backend's own elapsed milliseconds and a list of notes. `FfmpegBackend` is today's render, unchanged in output (same filter graph, same result line). `SmelterBackend` is new. `render` dispatches on `--backend`.

### The translation (pure, `crates/timeline/src/smelter.rs`)

`build_smelter_plan(timeline, size) -> SmelterPlan` and `SmelterPlan::requests(&files) -> Vec<Request>` where a `Request` is a method, a path and a JSON body. Nothing here runs a process or reads a file. It uses the same checks as the ffmpeg plan (`check_editable`, even size, size caps, non-empty video) and refuses the same things with the same messages (a generator other than the card, a reference it cannot render).

A neutral list of segments sits under both renderers' contract: for each item of the video track, its timeline start frame, its source start frame and its frame count, and for each audio item likewise. The ffmpeg graph's `trim=start_frame=A:end_frame=B` and the Smelter `seek_ms` / `offset_ms` / unregister time are both functions of these numbers, and a test checks that they agree.

Mapping, with `fps` the project rate, `ms(f) = f * 1000 / fps`, `W x H` the output size:

| Project | Smelter |
|---|---|
| Output | `mp4`, `ffmpeg_h264` (`preset=veryfast`, `crf=18`, `yuv420p`), AAC 48 kHz stereo, `movflags=+faststart`; process-wide `SMELTER_OUTPUT_FRAMERATE=frame_rate_arg(fps)` (one Smelter per render, so the rate is per render) |
| Video clip, source frames `s..s+n` at timeline frame `t` | an `mp4` input with `seek_ms=ms(s)`, `offset_ms=ms(t)`, `required: true`, unregistered at `ms(t+n)`; scene: `rescaler` (`mode: fill`, centred) over `input_stream`. The fill-and-crop of `--size` is the rescaler's job |
| Cut | a scheduled `update` at `ms(t) - 0.5 frame` that swaps the scene (evidence 3) |
| `Gap` | scene `view` with `background_color: "#000000FF"` and no audio |
| Bridge card | the PNG `reelctl` already draws, registered as an image, shown as `image` with the output size; the same bitmap-font card as ffmpeg |
| Punch-in (`scale s`, ramp `r`, clip-local `start a`, `end b`) | for each clip frame `n` the zoom `z(n) = 1 + (s-1) * min(clip((n-a)/r, 0, 1), clip((b-1-n)/r, 0, 1))`, the formula the ffmpeg `zoompan` uses; frames with the same `z` share one scene; `z = 1` is the plain scene; otherwise a `view` holding an absolutely positioned `rescaler` of size `W*z x H*z` at `top = -(H*z-H)/2`, `left = -(W*z-W)/2`. One scheduled update per frame of a ramp (12 for a 6-frame ramp pair), no Smelter transitions, so the zoom is sampled exactly where ffmpeg samples it |
| Audio clip | the same input as a video item with the same range (one input for both), else its own input; the mixer's `audio.inputs` lists the inputs playing at each frame, updated together with the scene |
| Silence generator | no mixer input (the mixer outputs silence) |
| End | `send_eos_when: {"all_inputs": true}` on video and audio; the "clock" input when the last item has no input |

The order of the requests is fixed: images, inputs, output, scheduled updates, scheduled unregisters, `start` (no reset: a server is started for one render). The same project gives the same requests.

### The backend (`crates/reelctl/src/smelter_backend.rs`, `smelter_client.rs`)

1. Resolve the binary: `--smelter`, else `SMELTER_BIN`, else `smelter` on `PATH`. None found, or not executable: exit 1, message names the two ways to point at it and where to get it.
2. Check media: every file is a local regular file (`local_media_file`, as ffmpeg's path).
3. Pick a free port by binding port 0 and releasing it; start the binary with `SMELTER_API_PORT`, `SMELTER_OUTPUT_FRAMERATE`, `SMELTER_OFFLINE_PROCESSING_ENABLE=true`, `SMELTER_START_WHIP_WHEP_SERVER=false`, `SMELTER_START_RTMP_SERVER=false`, `SMELTER_LOGGER_FORMAT=json`, `SMELTER_WEB_RENDERER_ENABLE=false`, stdout and stderr to a log file in a temporary directory. Wait for `GET /status`, up to 30 s; a child that exits first is an error that quotes the log's tail. A guard kills and reaps the child on every path.
4. Register in the fixed order, one HTTP call each, checking the status: a non-2xx is exit 1 with Smelter's own `message` and `stack`; an input of a project with audio whose response has `audio_duration_ms: null` is exit 2 ("Smelter reads AAC audio only: ... has none it can use; re-encode the audio (`ffmpeg -i in.mp4 -c:v copy -c:a aac out.mp4`)"). A media file Smelter cannot decode (HEVC, VP9) is the server's 500, surfaced with the same advice.
5. Connect `GET /ws` before `start`; call `start`; wait for `OUTPUT_DONE` for the output, at most `60 s + 10 x the timeline's length`. The server's log is then read: any `ERROR` line is exit 1 and quoted (evidence 5).
6. Check the file: `ffprobe` counts the video frames; a count that differs from the plan is exit 1 ("Smelter wrote N frames, the project has M"; the file is not kept). Then rename to `--out`.
7. Print the same `render:` line as ffmpeg with `smelter_ms=` in place of `ffmpeg_ms=`, then the notes.

Exit codes: 0 done; 2 a bad project, size or option, or media Smelter cannot read (checked before or while registering); 1 Smelter missing, did not start, rejected a request, logged an error, timed out, or wrote the wrong number of frames. `--backend ffmpeg` (the default) is unchanged in behaviour and exit codes, and does not look for Smelter.

### What Smelter cannot reproduce (each is printed as a `render:` note when it applies)

- **The audio dip at a join.** Mixer volume is set per update and there is no audio transition, so the fade-out and fade-in of `fade_ms` is not applied. A cut that goes through sound can click; a cut into a pause (what `cut-silence` makes) does not.
- **The card font and picture** are the same PNG as ffmpeg's, so they agree to within encoding; Smelter's own `text` component is not used (it needs a system font and would not match the bitmap font).
- **Colour of untagged sources** (evidence 8). The note is printed when a source has no colour tags.
- **Audio codec and picture codec limits** of Smelter's mp4 input (H.264 and AAC only): refused, not rendered wrongly.
- **Scaling filters differ** (Smelter's GPU resampling against ffmpeg's bicubic): a mean pixel difference above the encoding noise at any size other than the source's, and in the zoom.
- **Encoder:** the same x264 through the same system FFmpeg, but a different GOP and rate-control state: files are not byte-identical and are never expected to be.

## Boundaries

- **Always:** the default backend is ffmpeg and its output does not change; Smelter is a child process killed on every path; the same project gives the same requests; a mismatch of frames, a dropped audio track or a server error fails and says so; no file from Smelter is trusted without the frame check.
- **Ask first:** adding a Cargo dependency for HTTP or WebSocket; making Smelter the default; binding Smelter to a fixed port; starting it with the web renderer, WHIP, WHEP or RTMP enabled.
- **Never:** download, vendor or commit Smelter; run it against a project's media over `http://`; leave a server running; claim the two backends agree where they do not.

## Testing Strategy

- **Pure translation, unit tests** (`smelter.rs`): one clip; a cut (second clip's offset, the swap time half a frame early, the mixer list); a gap; the card; punch-in sampled per frame against the ffmpeg formula (ramp up, hold, ramp down, a punch-in that starts before its clip, `z = 1` frames); NTSC and 24 fps frame rates; the clock input present only when the project ends without an input; audio and video items with the same range share an input and different ranges do not; a project without audio has no audio block; every refusal the ffmpeg plan has; determinism; the fade note.
- **Agreement test:** for projects made with the real edit operations (`cut_silence`, `punch_in`, `attach_bridge`), the segments read from the ffmpeg filter graph's `trim=` values and the segments read from the Smelter requests (`seek_ms`, `offset_ms`, unregister time) are the same list.
- **Client and lifecycle, with a loopback fake** (127.0.0.1 only, in the test process): the HTTP client sends the JSON it was given and returns a non-2xx body; a WebSocket frame sequence is read to `OUTPUT_DONE` (small, large, fragmented-by-TCP frames); the backend registers in the fixed order, sends `start` last, surfaces a 500's message, refuses `audio_duration_ms: null`, reads the JSON log for `ERROR`, times out, and kills its child on every path. The child is a one-line shell script that `exec sleep`s, with the fake server on the port the test passes in; a missing binary, a binary that exits at once, and one that never serves are tested with real shell scripts.
- **CLI end to end, no Smelter:** `reelctl render --backend smelter` with `SMELTER_BIN` pointing at a missing file exits 1 and names `SMELTER_BIN`; `--backend ffmpeg` and no flag render as before and never look at `SMELTER_BIN`; an unknown backend is exit 2. The child's environment has `REELCTL_WHISPER_MODEL`, `REELCTL_TRANSNET_WEIGHTS`, `ORT_DYLIB_PATH`, `OPENROUTER_API_KEY` and `SMELTER_BIN` removed unless the test sets it.
- **Real Smelter, by hand:** `fixtures/smelter_agreement.sh` renders `draft12` and `textured_cuts12` (AAC copies) with both backends and prints frame counts, per-segment mean absolute luma difference against the ffmpeg render, and a noise floor (the ffmpeg render against the source frames). The numbers, the date and the machine are in status. It is not a test: it needs a binary the repository does not hold, and a test that returns early when the binary is absent would report a pass it did not earn.
- **Mutation pass** on the pure translation by hand, in scratch copies; the survivors are listed in status.

## Open questions (recommended answers taken)

1. A Cargo dependency instead of a process? **No** (ADR-023).
2. Make Smelter the default backend? **No**, ffmpeg stays default until a measured gap justifies it.
3. Reproduce the audio dip with stepped volume updates? **No**, not in this slice; stated as a note.
4. Support HEVC or PCM sources by transcoding first? **No**: refused with the one-line `ffmpeg` advice; a silent transcode would hide a cost and a quality change.
