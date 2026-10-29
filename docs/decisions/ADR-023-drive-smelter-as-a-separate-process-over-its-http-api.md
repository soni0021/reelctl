# ADR-023: Drive Smelter as a separate process over its HTTP API, behind a trait, with ffmpeg the default

## Status
Accepted

## Date
2026-10-29

## Context
planer.md names software-mansion/smelter (a wgpu compositor) as the renderer of the editing engine, in place of headless-browser rendering. [ADR-019](ADR-019-edit-an-opentimelineio-project-with-our-own-subset-and-render-with-ffmpeg.md) built the first slice with an ffmpeg render and left Smelter for a later slice. This is that slice, written under the user's "p1-p3" and not reviewed line by line.

What Smelter is, checked on 2026-10-29 from its repository, its releases, smelter.dev and the `smelter-api` source, and then run:

- A toolkit "for real-time, low latency, programmable video and audio mixing" by Software Mansion, formerly `live-compositor`. It runs as a server (a `smelter` shell wrapper that runs `dependency_check` and `exec`s `smelter_main`; a Docker image; or built from source) with an HTTP API (default port 8081) and a WebSocket event stream. It is not published as an embeddable crate: `smelter-core`, `smelter-render` and `smelter-api` are the server's internal workspace crates.
- v0.6.0 (2026-06-09) has a prebuilt `smelter_darwin_aarch64.tar.gz` (19 MB; the sha256 matched the digest GitHub shows). On the Apple M3 here it started, chose "Apple M3 ... Metal", and rendered. It links Homebrew's FFmpeg dynamically (`@rpath/libavcodec.62.dylib`, so FFmpeg 8; here 8.1) and does not bundle it.
- **Licence: custom, not MIT.** GitHub reports `NOASSERTION`. Offline processing is under terms "analogous to the permissive MIT license"; real-time processing is limited to organisations of 50 people or fewer and under 10,000 concurrent viewers (90-day trial, then a commercial licence from $1,000 a month); offering a SaaS with programmatic video control, or bundling it in a distributed product, needs a commercial licence; the licensor may name the user in marketing. `reelctl` uses offline mode only, on the user's machine.
- Offline mode (`SMELTER_OFFLINE_PROCESSING_ENABLE=true`) renders faster than real time; the MP4 input reads H.264 and AAC only.

## Options
- **(a) Drive an installed Smelter over HTTP.** No new heavy dependency; the user installs it; fails closed if absent; never vendored.
- **(b) A Cargo dependency on the Smelter crates.** They are workspace-internal, so this would be a git dependency on a repository whose crates are not published, pulling a GPU stack and FFmpeg 8 development libraries into every `cargo build` of a CLI that so far builds with `ffmpeg-next` alone; and the licence above is not one to link into a distributed binary. Not built; not measured (a build was not attempted: the first two facts rule it out).
- **(c) Spec, trait and fake only.** Not needed: the binary runs here, so the translation could be verified instead of guessed.

## Decision
Option (a).

- `crates/timeline` gains `smelter.rs`: a pure translation from a project to an ordered list of Smelter API requests (`build_smelter_plan`, `SmelterPlan::requests`), tested without a server. `crates/reelctl` gains `render_backend.rs` (the `RenderBackend` trait, `FfmpegBackend`), `smelter_backend.rs` (resolve the binary, start one server per render on a free port, send the requests, wait for `OUTPUT_DONE`, read the log, trim, check), and `smelter_client.rs` (about 250 lines of HTTP and WebSocket over `std::net`, plus its tests; loopback only).
- `reelctl render --backend ffmpeg|smelter` (default `ffmpeg`, unchanged) and `--smelter <path>` / `SMELTER_BIN`, else `smelter` on `PATH`. No Smelter: exit 1, naming both ways to point at one and `--backend ffmpeg`. Media Smelter cannot read: exit 2 with the one-line `ffmpeg` fix.
- No new Cargo dependency, no download, nothing vendored.

The facts that shaped the translation, all measured (spec: [smelter-backend](../specs/smelter-backend.md), numbers: [status](../status.md#smelter-backend-2026-10-29)):

- A seek and an input's offset are exact to the frame: they are properties of the queue's data, not of events.
- A scheduled scene update is not. Smelter applies it to the frames already queued for the renderer, so a cut, a zoom step or the card lands on its frame in most runs and one or two frames early in others (about a third of runs under load). The mixer updates and input unregisters are the same kind of event, so the sound can shift too; only the audio durations were measured. Frame counts at the end of the output varied by up to four frames. The backend therefore asks for a few frames too many (the "clock" input outlasts the timeline by four frames) and trims to the exact count with `ffmpeg -c copy`, and fails if the count is then wrong. The scene-change jitter has no fix at this layer and is printed as a note on every render that has a scene change.
- A scheduled update that is wrong is only logged by the server (`ERROR`), never returned to the caller; the backend reads the JSON log and fails on any `ERROR` line. An mp4 with PCM audio registers with `audio_duration_ms: null` and plays no sound; the backend refuses such a project.
- The audio dip at a join cannot be reproduced (no audio transition), and untagged sources are read as BT.709, so their colour differs from the ffmpeg render. Both are printed as notes.

## Consequences
- ffmpeg stays the default and is the only backend that meets "frame-exact". Smelter is opt-in for the day a GPU compositor is wanted (manimgx overlays, RobustVideoMatting cutouts, shaders), at the cost of a separate install, FFmpeg 8, a process per render and a measured slowdown on a project of many pieces (status).
- **Network exposure:** the server listens on all interfaces (`*:port`), and the documented environment has no bind-address variable. For the length of a render, anything that can reach the machine can call an API that writes files. The port is random, the server lives for one render, and WHIP, WHEP and RTMP servers are off. Run it on a trusted network, or use ffmpeg.
- **Supply chain:** the binary is third-party code run with the user's rights. `reelctl` never fetches it; the docs give the release URL and say to check the published sha256.
- **FFmpeg coupling:** an FFmpeg upgrade that changes the libavcodec major version breaks Smelter 0.6.0 until it is rebuilt or updated; the error text from the server's log is shown with a hint.
- **API drift:** the request shapes are those of 0.6.0 (`deny_unknown_fields` on every type, so a changed field name fails loudly at registration). Re-run `fixtures/smelter_agreement.sh` after a Smelter upgrade.
- The overlay track (chart overlays, ADR-022) landed on `main` after this backend was written, so at the merge the Smelter backend was made to refuse a project with an overlay track (exit 2, naming `--backend ffmpeg`) instead of rendering it without the charts; a unit test pins it. Teaching Smelter to draw them is a later slice.
- Reversible: removing the backend is deleting `smelter.rs`, `render_backend.rs`, `smelter_backend.rs`, `smelter_client.rs`, `smelter_fake.rs`, the CLI test and the fixture script, and the `--backend` flag.
- **Environment:** the server is started with the parent's environment minus `OPENROUTER_API_KEY` and the `REELCTL_*` model paths. Other variables (a shell profile's secrets) are still inherited by a third-party binary; run it from a clean environment if that matters.
- **Not verified:** the written file's sound. The backend counts video frames only; the audio check is at registration (Smelter heard audio in each input), not on the output.
