# To do on the Mac

The checks this cloud container cannot make. It has no Swift toolchain, no VideoToolbox, no `say`, no `~/models`, no `Reels/`, no ONNX Runtime or weights, and no manimgx or Smelter.

Run them on the M3 from the repo checkout, at the latest `main`, and send back what each one printed. Each check says what to report.

## 0. Let the cloud session build on this Mac (optional, but fastest)

- [ ] Turn on Remote Control for this checkout: in the Claude desktop app, Settings, Claude Code, Remote Control, add the `reelctl` folder; or run `claude remote-control` in it. The cloud session can then build the Swift app and run these checks itself.

## 1. Build and install the app

- [ ] Have the toolchain: `xcode-select --install` (or Xcode 16+), `brew install ffmpeg pkg-config`.
- [ ] `make -C macos test` reports that the `ReelCoreTests` pass. Report the summary line, and any compile errors in full.
- [ ] `make -C macos app dmg` makes `macos/dist/ReelStudio-<version>.dmg`. Report the path and size.
- [ ] Open the `.dmg` and drag ReelStudio to Applications. The first launch needs a right-click, Open, Open. Report whether Gatekeeper allowed it.
- [ ] The onboarding sheet asks for a workspace (keep `~/Movies/ReelStudio`) and Doctor lists ffmpeg as found.

## 2. App checks (report pass/fail per line)

- [ ] Library: import a clip made by `bash fixtures/generate.sh draft12 /tmp/draft.mp4` and decompile it. The lanes show cuts and silence. Clicking a cut marker shows that exact frame.
- [ ] Projects: init, cut-silence, render, then Undo (revert) and render again. History shows every checkpoint and nothing is deleted.
- [ ] Projects: "Try another version" (branch), then an edit on the branch, and the first project is unchanged.
- [ ] Jobs: quit the app during a render of `bench60`, reopen it, and the job is still running and then finishes.
- [ ] Understand: Plan shows a bound and does not spend anything. Do not press Spend unless you want a real call.
- [ ] Storage: a dry run shows the bytes, and the real collection only removes what it listed.
- [ ] Settings: set an API key, quit, reopen, and it is still set. `grep -r "sk-or" ~/Movies/ReelStudio ~/Library/Preferences/*ReelStudio*` finds nothing.
- [ ] Make reels on `bash fixtures/generate.sh lecture70 /tmp/lecture.mp4`, with an outlier DNA from Library, ends with reels after the cost sheet. A real call needs your yes.

## 3. Engine checks that only run on macOS

- [ ] Whisper: `REELCTL_WHISPER_MODEL=~/models/whisper/ggml-apex-hinglish-q8_0.bin cargo test -p reelctl --test whisper_fixture -- --include-ignored`. Report the result line.
- [ ] VideoToolbox: `cargo run --release -p reelctl -- decompile --input <an HEVC clip from Reels/> --out /tmp/hevc.json`, then report `decode_note`, which should say `Apple media engine`.
- [ ] RVM: run the manual commands in [docs/status.md](../docs/status.md#rvm-presenter-2026-10-29) with your `rvm_mobilenetv3_fp32.onnx` and `ORT_DYLIB_PATH`. Report the ms per frame.
- [ ] Reframe: `reelctl transform reframe --follow presenter` on a clip from `Reels/`. Watch whether the crop keeps the presenter in frame through cuts.
- [ ] Real lecture: `reelctl make-reels --input <lecture from Reels/> --outlier <dna> --target-url <url> --out-dir /tmp/mr` with the Apex whisper model, the RVM weights, and one live `understand` call after your yes. Report `make_reels.json`'s status and the notes.
- [ ] Smelter: `fixtures/smelter_agreement.sh` against your installed Smelter. Report its output.
- [ ] manimgx: `reelctl overlay add-chart` with the real manimgx (`MANIMGX_BIN`) on a fixture project, then render. Report whether the chart appears over exactly its frames.

## 4. Release (only when you want one)

- [ ] `git tag v0.1.0 && git push origin v0.1.0` runs `.github/workflows/release.yml` on a macOS runner and attaches the `.dmg` to a Release. On a private repo the macOS minutes count 10x.
