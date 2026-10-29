# Projects (the editor)

Screen 3 of [docs/specs/mac-app.md](../../../../../docs/specs/mac-app.md). A project is a `project` subject; every operation is a job on it that takes the current `.otio` as input `project`, so every edit is a checkpoint.

| File | What it holds |
|---|---|
| `ProjectsScreen.swift` | `ProjectsScreen` (the entry point, no-argument init), the project list, the toolbar (Undo, Try another version, one button per operation), the inspector and the layout. |
| `ProjectEditor.swift` | `ProjectEditor`, the `@Observable` state of the open project: checkpoints, the `.otio` shown, the latest render and QA run, the selected range, the running job, and the actions (submit, watch, undo, revert, branch). |
| `ProjectCatalogue.swift` | The Swift mirror of the catalogue entries in `crates/studio/src/commands.rs` that the editor submits: each operation's argument keys and kinds, and its input roles. |
| `ProjectArgForm.swift` | The form values, how they become typed `args`, the control for each argument kind, and how a refused field is found in an error message. |
| `ProjectSourcePicker.swift` | The picker for a DNA or understanding input: an imported file, a subject's head `out`, or the project's recorded DNA. |
| `ProjectOperationForm.swift` | The inspector form for one operation. |
| `ProjectNewSheet.swift` | The New project sheet: `timeline.init` or `timeline.from-clip`. |
| `ProjectLanes.swift` | The lanes read from the `.otio`: clips, gaps and the card, punch-ins, captions, chart overlays and the reframe, plus one note line per recorded edit. |
| `ProjectTrackView.swift` | The track view: blocks sized by frame count, zoom, and range selection by dragging. |
| `ProjectHistoryAndOutputs.swift` | The checkpoint list, the render player and the QA gallery. |

## How the shown state is found

A checkpoint's label is the command of the job that made it.

- **A copy.** For `revert to #N` and `branch of #N`, the state is checkpoint N's.
- **No project file.** A `render` or `qa.inspect-boundaries` checkpoint leaves the project as its parent had it. A render's `out` is the mp4, and QA has no `out`.
- **An edit.** Any other checkpoint with an `out` holds the `.otio`.

Undo reverts to the parent of the edit shown. After a render, Undo therefore removes the last edit, not just the render. After a revert, Undo goes one step further back.

The render plays in the app. A label says when it was made from an earlier state of the project than the one shown.

Shared-code needs are in [NOTES.md](NOTES.md).
