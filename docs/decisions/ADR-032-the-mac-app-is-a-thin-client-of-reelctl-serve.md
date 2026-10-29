# ADR-032: The Mac app is a thin client of `reelctl serve`

Status: Accepted (2026-10-29)

## Context

The user wants every `reelctl` feature in a native Mac app (SwiftUI, a team `.dmg`, ad-hoc signed). The service ([studio-service](../specs/studio-service.md), ADR-028) already owns the queue, the store, the checkpoints, the cost gate and the command catalogue. A second implementation of any of these in Swift would drift from the Rust one.

## Decision

- The app talks only JSON-RPC to `reelctl serve` over the workspace's Unix socket, through a Foundation-only `ReelCore` library. It never builds argv, parses result lines or touches `.reelstudio/` except to open stored outputs read-only by the paths the service returns.
- `reelctl` ships inside the app at `Contents/MacOS/reelctl`, built from the same commit, so the client and the service always match. The app starts the service when its socket does not answer, and sends `shutdown` on quit only when no job is queued or running.
- The app is a Swift package built with `swift build` (no Xcode project). It has no third-party Swift packages, is not sandboxed, targets arm64 and macOS 15+, and is ad-hoc signed and not notarized.
- The API key lives in the Keychain and goes to the service only inside `understand.run`.

## Consequences

- Every feature gap is a service gap first. For example, `dataset build` is not a job command yet, so the Datasets screen cannot run it.
- The app cannot be compiled in the cloud container, which has no Swift toolchain. It is built on the user's Mac over Remote Control, or by the user.
- An unnotarized app needs one right-click Open on each Mac, documented in `macos/README.md`.
