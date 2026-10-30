# ReelStudio (the Mac app)

A native SwiftUI app for every `reelctl` feature, a thin client of `reelctl serve`. The spec is [docs/specs/mac-app.md](../docs/specs/mac-app.md), and the decision is [ADR-032](../docs/decisions/ADR-032-the-mac-app-is-a-thin-client-of-reelctl-serve.md).

**Status:** written but not yet compiled. The cloud session that wrote it has no Swift toolchain. The first build on a Mac is the first check; see [tasks/mac-todo.md](../tasks/mac-todo.md).

## Build

You need macOS 15+ on Apple silicon, Xcode 16 (or its command line tools), Rust (`cargo`), and `brew install ffmpeg pkg-config`.

```bash
make -C macos test
```

```bash
make -C macos dmg
```

- `test`: runs `swift test`, the `ReelCoreTests`.
- `dmg`: builds `reelctl` (release) and the app, assembles `dist/ReelStudio.app` with `reelctl` in `Contents/MacOS/`, signs it ad hoc, and writes `dist/ReelStudio-<version>.dmg`.

## Install

Open the `.dmg` and drag ReelStudio to Applications. The app is ad-hoc signed and **not notarized**, so on each Mac the first launch needs a right-click on the app, then **Open**, then **Open** again. After that it opens normally.

On first launch the app:
1. asks for a workspace (default `~/Movies/ReelStudio`);
2. runs Doctor;
3. starts `reelctl serve` for that workspace.

ffmpeg comes from Homebrew. Models, weights, manimgx and Smelter are yours to install; set their paths in Settings. The whisper model is filled in for you the first time if `~/models/whisper/ggml-apex-hinglish-q8_0.bin` exists (Clear in Settings keeps it empty); the other paths stay explicit.

## Layout

| Path | What it is |
|---|---|
| `Package.swift` | The `ReelCore` library, the `ReelStudio` app, and `ReelCoreTests` |
| `Sources/ReelCore/` | The JSON-RPC client over the Unix socket (`RPCClient`), the typed API (`Studio`), the models, `edit_dna.json`, `understanding.json` and diff reports, the daemon launcher, the Keychain (`KeyStore`), the `.cube` reader, and `Playable` (a typed link for extensionless stored files) |
| `Sources/ReelStudio/App/` | The app, the window and sidebar, and `AppModel` (connection, jobs, live events) |
| `Sources/ReelStudio/Shared/` | Shared views: pickers, job progress, the frame-exact player, the paid-call sheet |
| `Sources/ReelStudio/Screens/` | One folder per screen group: Analysis (Library, Compare), Reels (the persistent output screen), Projects, Look, Presenter, Understand, Datasets, Overlays, MakeReels, Service (Jobs, History, Storage, Settings, onboarding, the ⌘K palette) |
| `docs/screens/` | Each screen group's file list and open notes |
| `scripts/` | `make-app.sh` and `make-dmg.sh` |
| `Resources/Info.plist` | The bundle's plist (the version is filled in from `Cargo.toml`) |

## Release

Push a tag `v*` to run `.github/workflows/release.yml` on a macOS runner. It runs the Rust tests and `make -C macos test dmg`, then attaches the `.dmg` to a GitHub Release. The repo is private, so macOS minutes count ten times against the Actions quota.
