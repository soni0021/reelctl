# Constraints

Last reviewed: 2026-10-18 (floor-only bootstrap; numbered dimensions deferred for a live interview)

This file is the project quality bar. Tightening is silent; loosening must be loud and reviewed.

Non-interactive install applied **the floor only**, per `constraint-driven-development`. Coverage, security scanners, performance budgets, and accessibility numbers were **not** invented. Flag for a human: run the four-question interview in that skill before adding numbered dimensions or installing extra checkers. Do not weaken this file to make a change pass.

## Floor (always enforced, no setup required)

- No new suppression comments: `@ts-ignore`, `eslint-disable`, `# noqa`, `# type: ignore`, `#[allow(...)]` without a documented reason owned in Exceptions
- No unimplemented stubs: `todo!()`, `unimplemented!()`, `throw new Error("Not implemented")`, empty `catch {}`
- No skipped or deleted tests without a reason in the commit message
- No secrets in source
- This file does not get weakened to make a change pass

## Enforced with numbers

| Dimension | Rule | Checked by | Runs at |
|-----------|------|-----------|---------|
| Compile | Workspace type-checks | `cargo check` (once `Cargo.toml` exists) | every edit |
| Tests | Suite green for the change | `cargo test` (once tests exist) | task end |

These two rows exist so the floor is not the only bar. They use the repo’s own toolchain; they are not extra product scope. Do not add npm/Python checkers here unless the stack actually uses them.

## Measured, not yet enforced

| Metric | Today | Direction |
|--------|-------|-----------|
| Numbered quality dimensions (coverage, cargo-audit, clippy as CI gate) | not measured | interview then ratchet; do not invent 80% coverage on empty crates |

## Exceptions

| ID | Rule | Path | Reason | Owner | Expires |
|----|------|------|--------|-------|---------|
| — | — | — | none yet | — | — |
