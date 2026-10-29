# ADR-033: `serve` jobs run in their own process group and never open a network port

Status: Accepted (2026-10-29). It adds to ADR-028. It also corrects the reasoning in ADR-030's consequence about `Tracked`, which was edited in place on 2026-10-29: the guard drops after `wait` returns, not before, and this ADR records that correction.

## Context

The task 6 audit found that a cancelled job's SIGKILL reached only the child `reelctl`, not the ffmpeg, whisper or manimgx processes under it. It also found that serve's own forced exit could not reach its children, and that `jobs.submit` could start a Smelter render, whose server listens on every interface. That broke the spec's rule that the Unix socket is the only listener.

## Decision

- Each job's child `reelctl` starts in its own process group (`process_group(0)`). Cancel sends SIGTERM to the group, then SIGKILL to the group after 5 s, through `nix`'s safe `killpg`. The child is registered with `progress::track`, so a forced exit of serve signals it too. An exit 130 while serve itself is stopping counts as a cancel.
- `jobs.submit` refuses `render` with `backend: "smelter"` or a `smelter` path. A Smelter render stays a command-line action.
- A live `understand` job that was stopped before it finished gets a warning event: the call may already have been billed, up to the approved amount.

## Consequences

- A terminal's Ctrl-C reaches serve but not its jobs directly. Serve's own cancel path stops them.
- A SIGKILLed serve still leaves its jobs' groups running (nothing is left to signal them), and the restart marks them `interrupted`.
- The service cannot drive Smelter. The Mac app's render uses ffmpeg.
