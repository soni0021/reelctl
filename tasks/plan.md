# Implementation Plan: long-to-short clipper (`reelctl understand`)

Spec: [docs/specs/long-to-short-clipper.md](../docs/specs/long-to-short-clipper.md), approved 2026-10-28 ("invoke multiple agent and complete it") with the recommended answers. Branch: `feature/long-to-short-clipper`. Checklist: [tasks/todo.md](todo.md). Earlier plans are in [tasks/archive/](archive/comparative-diff/plan.md).

## Overview

A new crate, `understand`, plus a `reelctl understand` subcommand. The crate is pure and offline-testable (segmenter, frame plan, prompt and cost bound, clip rules, assembly); the parts that touch the network, the API key, ffmpeg, or the command line stay in the main session. Work is split so that independent pure modules are built in parallel worktrees by separate agents from a committed contract.

## Architecture Decisions

- **The contract is committed first** (`crates/understand` with every public type, the `VlmBackend` and `ModelCatalog` traits, constants named in the spec, and honest stub bodies). Agents build against it, cannot change it, and report if they need to.
- **`understand` does not depend on `decompile`.** Frames reach it as images with times, so `cargo test -p understand` never builds ffmpeg or whisper.cpp. `reelctl` connects them through a new `decompile::decode_frames_at`.
- **Parallel units, one module each:** `segments.rs`; `clips.rs`; `frames.rs` plus `request.rs`; `assemble.rs`. Each is a pure function over the contract types. Each agent works in its own worktree and branch, commits a compiling red commit then green, and touches only its own file(s). Docs, `tasks/`, and ticks are written at integration.
- **Main session builds:** `decompile::decode_frames_at`, `openrouter.rs` (the key), the `understand` subcommand with its exit codes (1–4) and `--dry-run`, the hidden `--replay` and `--catalog` flags, the end-to-end tests, `docs/understanding.md`, ADR-015, and the schema file.
- **No network in the default suite.** The end-to-end test spawns the `reelctl` binary, which cannot take a Rust trait object, so it uses `--replay <reply.json>` and `--catalog <catalog.json>` (hidden from `--help`, documented in `cli.md`). The `reelctl()` test helper also removes `OPENROUTER_API_KEY`, and AGENTS.md says so, so a shell that sourced `.env` cannot make a paid call from `cargo test`.
- **The key never leaves the environment.** `OPENROUTER_API_KEY` is read from the process environment, passed only as a header, and never printed, logged, or written. A test uses a dummy key and searches stdout, stderr, and the output file for it.
- **Every commit stages explicit paths, never `git add -A`.** `.env` holds a live key. It is ignored on `main`, and `git check-ignore -v .env` is checked on this branch before the first commit.
- **The live run is separate.** Success criterion 5 (Qwen3.5-9B and Muse 1.2 on the 28.6-min lecture) sends frames and a transcript to a hosted model and costs money, so it waits for its own yes. `--dry-run` on the real lecture is free and runs first, to quote the cost bound.
- **Baseline:** `cargo test --workspace` is 144 passed and 2 ignored before this work; the existing fixtures must not change.

## Task List

### Phase 1: Contract
- [x] Task 0: Crate skeleton, types, traits, constants, stubs (compiles, no warnings)

### Phase 2: Pure modules (parallel, one agent each)
- [ ] Task 1: Segmenter (`segments.rs`)
- [ ] Task 2: Clip rules (`clips.rs`)
- [ ] Task 3: Frame plan, sheets, JPEG, prompt, token estimate, cost bound (`frames.rs`, `request.rs`)
- [ ] Task 4: Assembly (`assemble.rs`)

### Checkpoint A
- [ ] Four branches merged into the feature branch; `cargo test -p understand` passes; `git check-ignore -v .env` still holds

### Phase 3: Wiring (main session)
- [x] Task 5: `decompile::decode_frames_at`
- [ ] Task 6: `openrouter.rs` (backend and catalog) and the key rules
- [ ] Task 7: `reelctl understand`: exit codes 1–4, `--dry-run`, `--replay`, `--catalog`, end-to-end tests, schema

### Checkpoint B
- [ ] `cargo test --workspace` passes offline; `--dry-run` on the real lecture gives a cost bound

### Phase 4: Close-out
- [ ] Task 8: Docs, ADR-015, audit, fixes
- [ ] Task 9: Live run on the lecture (needs the user's yes)

### Checkpoint C
- [ ] Criteria 1–4 and 6 verified; criterion 5 verified or marked pending; audit run; reviewed with the user, merged into `main`, and this plan moved to `tasks/archive/long-to-short-clipper/`

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| The API key is committed or printed | High | `.env` ignored on `main` before branching; explicit-path staging; a test that searches all output for a dummy key; the live test is `#[ignore]`d |
| The default suite makes a paid call | High | The test helper removes `OPENROUTER_API_KEY`; the backend is never constructed without a key (exit 3); the end-to-end test uses `--replay` |
| Parallel agents disagree on a shared type | Medium | The contract is committed first and read-only for agents; they report needs instead of editing it; integration merges only disjoint files |
| `ureq`/rustls fails to build or to verify certificates | Medium | Built before the contract commit; the first real request is the unauthenticated catalog fetch in `--dry-run`, which fails closed if TLS fails |
| The cost bound is wrong (token estimate) | Medium | It uses the highest price and 1,445 tokens per image, which overestimates; the actual cost is recorded next to it |
| A clip rule rewrites the model's choice silently | Medium | Rules flag or reject, and every change is listed in `adjustments`; tests for each rule |
| The model's times or quotes leak into the output | Medium | The reply type has no time except `t_s`, which is snapped to a frame time; quotes are copied from `segments`; a test compares every text field |
| `--dry-run` fetches the public listing, so it is not offline | Low | It says so, needs no key, and fails closed; tests use `--catalog` |
| Sheets lose detail the model needs | Low | `--sheet auto` prefers single frames and only tiles when the context requires it |
