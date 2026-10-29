# Screens/Analysis

The Library (spec screen 1, Reel analysis) and Compare (spec screen 2) screens of ReelStudio ([docs/specs/mac-app.md](../../../../../docs/specs/mac-app.md)). Not yet compiled: the cloud container has no Swift toolchain.

| File | What it holds |
|---|---|
| `LibraryScreen.swift` | `LibraryScreen`: the list of video assets, import (button or drop), Analyse (`decompile` on the file's `reel` subject, `whisper_model` from Settings when set), the player, the lanes, the filmstrip and the inspector |
| `AnalysisMarkers.swift` | The lanes (`AnalysisLane`) and the markers built from `edit_dna.json`: cuts, the payoff window, shots, silence gaps, words, loudness, visual events, zoom curves |
| `AnalysisLanesView.swift` | The Canvas that draws the lanes, a click to select a marker and seek, a drag to scrub |
| `AnalysisFilmstrip.swift` | Reads the `views` tree manifest, gets each PNG through `objects.path`, maps `timeline_view_cut_<NN>_f<frame>.png` to its cut; the thumbnail row |
| `AnalysisInspector.swift` | The selected marker's fields, and the run's measurements and notes |
| `AnalysisSupport.swift` | Shared helpers: subject lookup and creation, the newest checkpoint with given outputs, the whisper argument, frame-exact seek times |
| `CompareScreen.swift` | `CompareScreen`: draft, one to three outliers, `diff`, the players side by side, the results |
| `CompareParts.swift` | The outlier picker (an `edit_dna` asset or an analysed reel), the flag list, the Swift Charts chart of cuts per 5 s window, the metrics grid |

## Behaviour worth knowing

- **Subjects.** The catalogue has no link from an asset to its subject, so a video's `reel` subject is found by name (the asset's name), and created on the first Analyse. Compare's `diff` runs go on a separate `reel` subject named `compare · <draft name>`, so a report never becomes the head of the draft's own analysis. Compare's "analysed reels" list leaves those out.
- **Which analysis is shown.** The subject's head checkpoint when it holds an `out`, else the newest checkpoint that does.
- **Frame-exact seeks.** A marker with a frame index (a cut, a visual event) seeks to the middle of that frame, `(frame + 0.5) × 1000 / fps` ms, so the zero-tolerance seek lands on the frame itself even at 29.97 fps. Shots and zoom curves use the frame nearest their start. Silence and words seek to their start ms.
- **Clicks.** In a lane, a point marker within 6 pt wins over a range within 3 pt. A click on empty space, or on the ruler, seeks to that time. On the loudness lane it selects the loudness marker and seeks to the clicked time.
- **Filmstrip.** The `views` output's stored path is the tree's JSON manifest (`{kind, entries: [{name, hash, size, kind}]}`); each PNG's path comes from `studio.objectPath(hash)`. A thumbnail maps to its cut by frame, then by `<NN>`, then by order.

## Shared-code changes that would help (not made here)

- `DiffReport.RelativeFlag` has no `outliers_with_cut` or `outliers_counted`, and `DiffReport.Metrics` has no `words_source`, so the flag list can't say "2 of 3 outliers cut here", and the table can't mark an estimated words-per-minute. Add those fields to `ReelCore/Documents.swift`.
- `FramePlayer` never removes its periodic time observer (it has no `deinit`), and each screen keeps two players. Add a `deinit` that calls `player.removeTimeObserver`.
- A field linking an asset to its `reel` subject, in the catalogue or the subject record, would replace the lookup by name. Today two imported files with the same name share a subject.
