# Overlays screen

`OverlaysScreen` (docs/specs/mac-app.md, screen 8): chart overlays on a project.

| File | What it holds |
|---|---|
| `OverlaysScreen.swift` | The screen: project picker (kind `project`), the manimgx check (`doctor.check`, with a link to Settings when missing), Add chart (`overlay.add-chart`), Remove (`overlay.remove`), reload on every checkpoint. |
| `OverlayChartDraft.swift` | The chart spec as the form edits it (`ChartSpec` in `crates/scenes/src/spec.rs`: `kind`, `title`, `series`, `unit`, `start_ms`, `end_ms`, `position`, `width_pct`), its JSON, the client-side checks from docs/cli.md, and the writer to `$TMPDIR/ReelStudio-charts/`. |
| `OverlayChartForm.swift` | The form and the live JSON preview with its problems. |
| `OverlayList.swift` | Reads overlays loosely from the head `.otio` (the track marked `metadata.reelctl.overlays`, each clip's `metadata.reelctl.overlay`; captions from the timeline's `metadata.reelctl.captions`) and lists them with Remove. |

Add chart: the spec is written to the temporary folder, imported as a `chart_spec` asset (`model.importFile`), and submitted as `overlay.add-chart` on the project subject with `project = .hash(head out)` and `spec = .asset(id)`, plus `skip_inspect` and `timeout_s`. The span is in the spec; the command has no timing flags. Remove is `overlay.remove` with `id`, or `captions: true`.

Shared-code needs: see [NOTES.md](NOTES.md).
