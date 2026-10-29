# Presenter screen

`PresenterScreen` (docs/specs/mac-app.md, screen 5): `compositor.isolate-presenter` (input
`input`; args `mode`, `background`, `weights`, `downsample_ratio`; output `out`) on a
`presenter` subject, with before and after players.

| File | What |
|---|---|
| `PresenterScreen.swift` | The form, the Doctor check (on appear and again right before each submit), the submit, waiting, loading the `out` file, and the head checkpoint's cut-out when a subject is picked. |
| `PresenterOptions.swift` | `PresenterOptions` (mode, background colour as `#rrggbb` or an image/video path, optional downsample ratio, the typed args) and its form rows. `background` is sent only for `composite`. |
| `PresenterTools.swift` | What `doctor.check` must find (`rvm_weights`, `onnx_runtime`, and `ffmpeg`/`ffprobe` when listed) and the weights path to send: `ToolSetting.rvmWeights.value` when set (must be absolute), else Doctor's `rvm_weights` path. |
| `PresenterResults.swift` | The missing-tools `ContentUnavailableView` (Settings button sets `model.selection = .settings`), the side-by-side players, and the per-frame count from `model.progress`. |

The weights are always sent as the `weights` argument: the job's environment is built from
scratch and the catalogue passes only `ORT_DYLIB_PATH` through, so `REELCTL_RVM_WEIGHTS` would
not reach the child.

Shared code used: `AppModel`, `ToolSetting`, `AssetPicker`, `SubjectPicker`, `JobProgressView`,
`FramePlayer`, `PlayerView`, `FieldRow`, `chooseFile`, `UTType.videoTypes`.

Shared-code needs: see `NOTES.md` (the `rgba` mode cannot run through the service yet).
