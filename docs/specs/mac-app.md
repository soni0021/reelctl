# Spec: ReelStudio, the Mac app

Status: **approved 2026-10-29** by the user ("Yes, approved"), from the plan they had seen, with the instruction to build without stopping and to record any deviations here. Milestones M2 to M8 of the ReelStudio plan. Branch: `claude/keen-clarke-1a8w5h`, merged to `main` per milestone.

## Objective

Every `reelctl` feature, at equal depth, in one native Mac app for the user's M3 MacBook and their team. The app is a thin client of `reelctl serve` ([studio-service](studio-service.md)). It starts the service, sends JSON-RPC requests, shows the live events, and opens the stored outputs. It never builds a command line, parses a result line or writes into the store itself ([ADR-032](../decisions/ADR-032-the-mac-app-is-a-thin-client-of-reelctl-serve.md)).

## Tech stack

- **Swift 6 package** in `macos/`, built from the command line (`swift build`, no Xcode project). It has two targets:
  - **`ReelCore`** (a library): Foundation, plus Security for the Keychain. It holds the JSON-RPC client over the Unix socket (POSIX sockets), the Codable models, the daemon launcher, the Keychain wrapper and the `.cube` reader. Tested by `ReelCoreTests` (XCTest) against an in-process fake socket server.
  - **`ReelStudio`** (the executable): SwiftUI, AVKit, Charts, Core Image and AppKit.
- **Platform:** macOS 15 or later, arm64 only. Not sandboxed, because it starts `reelctl` and reads the user's video files where they are.
- **Packaging:**
  - `macos/scripts/make-app.sh` assembles `ReelStudio.app`, with `reelctl` (built `cargo build --release`) in `Contents/MacOS/` beside the app binary, `Info.plist`, and an ad-hoc signature (`codesign --sign -`), inner binary first.
  - `macos/scripts/make-dmg.sh` makes `macos/dist/ReelStudio-<version>.dmg` (`hdiutil create -format UDZO`, with an `Applications` link).
  - `make -C macos test app dmg`.
  - The version comes from the workspace `Cargo.toml`.
- **Not notarized.** On first open the user right-clicks, chooses Open, and confirms once. That is documented in `macos/README.md`.
- **No third-party Swift packages.** The waveform is drawn from the PCM that `reelctl` already measures, or from a downsampled read of the audio through AVFoundation, rather than with DSWaveformImage.

## Structure

- **The service.** On launch the app reads the last workspace (default `~/Movies/ReelStudio`), connects to `<workspace>/.reelstudio/reelctl.sock`, and if nothing answers, starts `Contents/MacOS/reelctl serve --workspace <dir>`. It waits up to 10 s for the socket.
  - When the app quits with no job queued or running, it sends `shutdown`. Otherwise the service keeps running and the app reconnects next time (the spec's resolved question 2).
  - The service's environment is the app's, plus `/opt/homebrew/bin` on PATH, and plus the tool paths set in Settings: `MANIMGX_BIN`, `SMELTER_BIN`, `ORT_DYLIB_PATH`, `REELCTL_WHISPER_MODEL`, `REELCTL_TRANSNET_WEIGHTS` and `REELCTL_RVM_WEIGHTS`. `OPENROUTER_API_KEY` is never in it.
- **The window.** A `NavigationSplitView`: a sidebar, a content area and an `.inspector`.
- **The sidebar sections:**
  - **Library:** imported reels and their analysis.
  - **Make reels**
  - **Projects:** the editor.
  - **Compare**
  - **Look**
  - **Presenter**
  - **Understand**
  - **Datasets:** with Eval.
  - **Overlays:** chart overlays, part of Projects but with its own section for the chart spec editor.
  - **Jobs**
  - **History**
  - **Storage**
  - **Settings:** with Doctor.
- **State.** One `@Observable` `AppModel` holds the connection, the workspace info, the subjects, the jobs and the latest events. It subscribes once (`events.subscribe`) and updates jobs from `job.status`, `job.progress` and `checkpoint.created` notifications. Screens read it and call `ReelCore`'s typed `Studio` API.

## Screens

1. **Library (Reel analysis).**
   - Import a video (drag and drop, or an open panel). Run `decompile`, with whisper when a model is set in Settings.
   - Show an AVPlayer over lanes: cuts, shot colours (`shot_histograms` means), silence gaps (gaps over 180 ms marked as dead air), words, loudness (the integrated value and a bar), `visual_events` and `zoom_curves`.
   - Clicking a marker seeks to that exact frame (`seek(to:toleranceBefore: .zero, toleranceAfter: .zero)`). Scrubbing uses the chase-time pattern: one seek in flight, and the latest target kept.
   - The `timeline_view` filmstrip pictures (the `views` tree) are a row of thumbnails; a click seeks to that cut.
   - The inspector shows the selected marker's fields, `signal_ms`, `asr_backend`, `cut_detector`, the notes and `decode_note`.
2. **Compare.**
   - A draft video against 1 to 3 outlier DNAs (`diff`).
   - The flag list (dead air, static window, missing payoff cue, rhythm gap, pause density); a click seeks the draft.
   - A Swift Charts chart of the cuts per 5 s window (draft against outliers), and the metrics table.
   - Players side by side.
3. **Projects (the editor).**
   - Create a project with `timeline.init` or `timeline.from-clip`.
   - A track view drawn from the head `.otio`: clips, gaps, punch-ins, the card, captions and overlays, read from `metadata.reelctl`.
   - The operations are on the toolbar and in the inspector forms, built from the command catalogue's arguments: cut-silence, punch-in (a range selected on the track fills its start and end), auto-punch-in, reframe, attach-bridge, captions with the full style form and presets, overlay remove, render, and qa inspect-boundaries.
   - Every operation is a job on the project subject, taking the head `.otio` as input. Undo is `checkpoints.revert` to the parent, and "Try another version" is `subjects.branch`.
   - The render plays in the app. The QA gallery shows each join's views.
4. **Look.** Pick a draft and an outlier and run `look`. Preview the `.cube` live on the draft with Core Image (`CIColorCubeWithColorSpace`, the cube read by `ReelCore`'s reader) as a split wipe with a draggable divider. Reveal the `.cube` in Finder.
5. **Presenter.** `compositor.isolate-presenter`: output composite, matte or rgba; a background colour or image; per-frame progress from the events; before and after players. Without RVM weights, the screen says what is missing (Doctor) and does not submit.
6. **Understand (paid).** Pick a video and a DNA with words, plus the model and `every_s`. **Plan** calls `understand.plan` and shows frames, images and the bound. **"Spend up to $X.XXXX"** asks every time: a sheet with the amount, the model, and that frames and the transcript leave the Mac. It then calls `understand.run` with the key from the Keychain. The clip list shows hook, payoff and last lines, lengths and flags; a click plays the clip's range in the source.
7. **Datasets and Eval.** `dataset build` is not in the job catalogue (its inputs are folders), so it is listed as a gap until the service gains it. `eval` with a truth file and answers shows the metrics table.
8. **Overlays.** A chart-spec form (the JSON that `overlay add-chart` takes), `overlay.add-chart` on a project, and remove. It needs manimgx (Doctor).
9. **Make reels.** Drop a long video, pick 1 to 3 outlier DNAs, the target URL, the caption style and the clip count. Then `pipelines.make_reels` in its three stages: decompile, then the plan with the same paid-call sheet as Understand, then the reels. The result is one card per reel, with its step progress, the rendered reel playing, its diff flags and its notes (for example, the centre crop used for lack of weights).
10. **Jobs.**
    - The live queue grouped by class, with status, stage, a progress bar, the latest log lines, cancel, retry and the "cached" badge.
    - `after` chains shown as indented children.
    - "Copy as CLI command": the job's command and arguments as a `reelctl` line, with the input paths. It never includes a key.
11. **History.** For the selected subject, the checkpoint list (label, job, cached, time). Revert, Branch, and Compare any two (`checkpoints.diff`; for two `.otio` or two videos, side by side). Play plays a checkpoint's stored video in a sheet; Export saves it as an `.mp4` (or `.mov`) through a save panel; Reveal shows a video as `<hash>.mp4` through a typed link in the temp folder, so Finder opens it in QuickTime.
12. **Storage.** `store.stats` by subject and kind. Garbage collection: a dry run first, then a confirmation that shows the bytes freed, then `store.gc` with `dry_run: false`.
13. **Settings and Doctor.**
    - `doctor.check`, each tool with found, version and path, and a Choose… button that sets its path. The service restarts with the new environment when no job runs.
    - The API key: set, replace or delete, in the Keychain.
    - The workspace folder.
    - The queue limits (`workspace.set_limits`).
    - The default caption preset.
    - "Check for updates": only on a click. It reads the repo's latest Release through GitHub's API and opens its page. The repo is private, so this needs a token; without one it says so and opens the Releases page.

Across the app:
- a **⌘K command palette** over the sections and the main actions
- **drag and drop** of videos and DNAs onto Library, Compare and Make reels
- **error mapping:** `data.exit_code` 2 points at the field at fault, 3 opens the API key setting, and 4 opens the cost sheet

## Keychain

A generic password (service `ReelStudio`, account `openrouter`), written with `kSecUseDataProtectionKeychain`. If that fails for the ad-hoc signed build (unverified, a Mac check), it is written to the login keychain. The key is read only to pass it in `understand.run`, and is never logged, written to a file or shown back.

## Onboarding

On first launch a sheet:
1. asks for the workspace folder;
2. runs Doctor;
3. when ffmpeg is missing, gives `brew install ffmpeg` with a copy button;
4. says how to open an unnotarized app.

## Release

`.github/workflows/release.yml`, on a pushed tag `v*`, on `macos-15`:
1. `brew install ffmpeg pkg-config`
2. `cargo test`
3. `make -C macos test dmg`
4. attach the `.dmg` to a GitHub Release

It runs only on a tag. The repo is private, so macOS minutes count ten times against the Actions quota.

## Boundaries

- **Always:**
  - every action goes through `reelctl serve`;
  - every paid call shows its bound and needs a yes;
  - the key stays in the Keychain;
  - outputs are read from the store, read-only.
- **Ask first:**
  - a third-party Swift package;
  - sandboxing;
  - notarization.
- **Never:**
  - bundle ffmpeg, models, weights, manimgx or Smelter;
  - write into `Reels/` or into an imported file;
  - print or store the key.

## Testing

- **`ReelCoreTests`** (`swift test`):
  - request and response framing; notifications delivered while a call waits;
  - error mapping with `data.exit_code`;
  - decoding the models from JSON the Rust tests produce;
  - the `.cube` reader;
  - "Copy as CLI command" never containing a key.
- **The app:** `swift build` with no warnings, then the manual checks in [tasks/mac-todo.md](../../tasks/mac-todo.md).
- **Where it builds:** this cloud container has no Swift toolchain, so the Swift is compiled on the user's Mac, over Remote Control when it is attached, or by the user from the to-do list. Status says which code has been compiled.

## Success criteria

1. `make -C macos test app dmg` succeeds on the M3, and the `.dmg` installs and opens after one right-click Open.
2. On the fixture clips:
   - Library shows a decompile's lanes, and a click seeks to the frame.
   - A project edit, render, revert and branch all work from the UI.
   - Make reels takes a long video to reels after the cost sheet.
3. Quitting the app during a render and opening it again shows the job still running, then finished.
4. No key in any file of the workspace or in the app's preferences.
