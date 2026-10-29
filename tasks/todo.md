# Todo: ReelStudio, the Mac app (M2–M8)

Plan: [plan.md](plan.md). Spec: [docs/specs/mac-app.md](../docs/specs/mac-app.md). Mac checks: [mac-todo.md](mac-todo.md). Tick each box in the commit that completes it. Stage explicit paths; never `git add -A` (`.env` holds a key).

"Written" means the code exists but no Swift compiler has seen it yet.

## M2: Foundation
- [x] ReelCore, ReelCoreTests, the app shell, Jobs, History, Storage, Settings, onboarding and the palette: written
- [x] `make -C macos test app dmg`, the scripts, `Info.plist`, and the Release workflow: written
- [ ] Compiles, `swift test` passes, and the `.dmg` installs (on the Mac)

## M3: Analyse
- [x] Library and Compare: written
- [ ] Checked on the Mac

## M4: Edit
- [x] Projects: written
- [ ] Checked on the Mac

## M5: Look and Presenter
- [x] Written (RGBA output is a `.mov`, named by the service)
- [ ] Checked on the Mac

## M6: Understand and Datasets
- [x] Written (`dataset build` is not a service job yet: the screen shows the command)
- [ ] Checked on the Mac

## M7: Overlays
- [x] Written
- [ ] Checked on the Mac, with the real manimgx

## M8: Make reels
- [x] Written, on `pipelines.make_reels`
- [ ] Checked on the Mac
