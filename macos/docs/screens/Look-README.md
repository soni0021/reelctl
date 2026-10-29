# Look screen

`LookScreen` (docs/specs/mac-app.md, screen 4): pick a draft and an outlier video, run the
catalogue command `look` (args `size` "17"/"33"/"65" as text, `samples`, `decoder`; inputs
`draft`, `outlier`; output `out`, the `.cube`) on a `look` subject, and preview the cube.

| File | What |
|---|---|
| `LookScreen.swift` | The form, the submit (`model.perform { try await $0.submit(...) }`), reading the `out` file with `CubeLUT(url:)`, the head checkpoint's cube when a subject is picked, "Reveal in Finder" and "Save a copy…" (copies the read-only stored file's bytes to a new file). |
| `LookPreview.swift` | `LookPreviewPane` (a draft frame at a slider time, `AVAssetImageGenerator` with zero tolerance), `LookSplitWipe` (draggable divider: original left, graded right), `LookImaging` (one shared `CIContext`, `CIColorCubeWithColorSpace` with `inputCubeDimension` = size, `inputCubeData` = `cube.data`, sRGB colour space). |

Shared code used: `AppModel` (`perform`, `waitFor`, `output(of:role:)`, `headFile(of:role:)`,
`assets`, `refreshSubjects`, `banner`), `AssetPicker`, `SubjectPicker`, `JobProgressView`,
`FieldRow`, `formatMs`, `UTType.cube`, `UTType.videoTypes`; `ReelCore`'s `CubeLUT`.

Shared-code needs: none.
