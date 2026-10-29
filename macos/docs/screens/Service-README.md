# Service screens

The Jobs, History, Storage and Settings sections, the first-launch sheet and
the ⌘K palette of ReelStudio (docs/specs/mac-app.md, screens 10 to 13,
Onboarding, and "Across the app"). Every action goes through
`model.perform { try await $0.<Studio method> }`.

| File | What it holds |
|---|---|
| `JobsScreen.swift` | `JobsScreen`: the queue by class, the status filter, `after` chains as indented rows plus an "after #N" badge; `JobsFilter`, `JobTree`, `JobRowView`, `JobClassHeader` |
| `JobInspector.swift` | The selected job: arguments (pretty JSON), inputs, environment names only, result, checkpoint, live and stored events; Cancel, Retry (not for `understand`), Copy as CLI command (`jobs.argv`) |
| `HistoryScreen.swift` | `HistoryScreen`: a subject picker grouped by kind, the checkpoint table with the head marked; Revert (with a confirmation), Branch (name prompt, then focuses the new subject), Compare two, Reveal outputs |
| `CheckpointCompareSheet.swift` | `checkpoints.diff` as same / changed / only in A / only in B; a changed role opened side by side (two players for videos, two texts otherwise) |
| `StorageScreen.swift` | `StorageScreen`: `store.stats` totals, by kind, and a sortable table by subject; garbage collection as dry run, confirmation with the bytes freed, then the real run |
| `SettingsScreen.swift` | `SettingsScreen`: tool paths (`ToolPathRow`) and Apply (service restart), workspace, queue limits, the default caption preset (`captionPreset` in UserDefaults), Check for updates |
| `SettingsSections.swift` | `DoctorSection`, `DoctorRow`, `DoctorCatalog`, and `APIKeySection` (Set / Not set, SecureField, Save, Delete; the value is never shown) |
| `OnboardingView.swift` | `OnboardingView`: workspace, Doctor with `brew install ffmpeg` and Copy, how to open an unnotarized app, Done (sets `onboarded`) |
| `CommandPalette.swift` | `CommandPalette`: filtered sections and actions, arrow keys, Return, Escape |
| `ServiceSupport.swift` | Formatting (`ServiceFormat`), `serviceCopy`, `serviceReveal`, `ServiceBadge`, and `ServiceObjectKind` (sniffs a stored object's type; links a video object into the temporary folder with an extension so AVFoundation plays it) |

Shared code these files use: `AppModel`, `AppSection`, `ToolSetting` (App/),
and `StatusDot`, `FieldRow`, `FramePlayer`, `PlayerView`, `formatBytes`,
`formatMs`, `chooseFile`, `chooseFolder` (Shared/Components.swift). What they
would like changed elsewhere is in `NOTES.md`.

Not compiled yet: written in a Linux container with no Swift toolchain.
