# ADR-028: A local daemon over a Unix socket runs jobs as child reelctl processes

## Status
Accepted (with [studio-service](../specs/studio-service.md), 2026-10-29)

## Date
2026-10-29

## Context
The user wants a request/response API and a producer–consumer queue under the Mac app, and chose a Rust daemon over a Unix socket. Three facts constrain the design:
- `reelctl` already runs each step as a child process in `make-reels`, and that rule carries over to the daemon ([ADR-027](ADR-027-make-reels-runs-each-step-as-a-child-reelctl-with-a-minimal-environment.md)). One ONNX model per process is required (AGENTS.md).
- `reelctl understand` is the only command meant to use the network. The project avoids listening on any interface: the only exception, Smelter's port during a render, is documented as a limit.
- The app runs on the user's own Mac for one user.

## Decision
- **One daemon per workspace.** `reelctl serve --workspace <dir>` listens on `<dir>/.reelstudio/reelctl.sock` (0600, inside a 0700 directory) and checks each connection's peer uid. It never opens a TCP port.
- **The protocol** is JSON-RPC 2.0, one JSON object per line, with server-pushed notifications for job events.
- **Jobs are child `reelctl` processes**, built from a typed command catalogue (never a raw argv), with a from-scratch environment and `--progress json` on stderr. Pools per resource class (`heavy`, `light`, `network`) limit how many run at once.
- **No async runtime.** It uses std threads and condition variables.

## Consequences
- The same `reelctl` binary is the CLI, the daemon and every worker. There is no second implementation of any command, and a job can always be re-run by hand from its recorded argv.
- A process per job costs a few milliseconds each, which is small next to video work.
- Other local processes of the same user can reach the socket. That is the same trust boundary as the user's files. Other users cannot.
- Remote use, such as a team Mac mini, would need a tunnel such as SSH socket forwarding. It is not built in.
