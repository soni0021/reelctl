# ADR-031: A live `understand` call needs the plan's bound approved exactly

Status: Accepted (2026-10-29)

## Context

`reelctl serve` must never spend money without the user's yes for that amount (spec section 5, ADR-028). The command line already refuses a run whose cost upper bound is over `--max-cost-usd` (exit 4). The service splits the call into `understand.plan` (a `--dry-run` job: no key, no model call) and `understand.run`. The dry run prints the bound rounded to four decimals, so the printed figure can be up to $0.00005 under the bound `understand` computes again on the live run, and a cap equal to the printed figure could be refused for no reason.

## Decision

- `understand.plan` reports the printed bound plus $0.0001, rounded to four decimals: never under the computed bound.
- `understand.run {plan, approved_max_usd, api_key}` is refused unless the plan exists, is unused, is at most 15 minutes old, and `approved_max_usd` equals the reported bound (within 1e-9). More than the bound would approve a spend the user was not shown; less would be refused by `understand` itself, so it is refused before any child runs (`data.exit_code` 4). A missing key is refused with `data.exit_code` 3.
- The approved amount is passed as `--max-cost-usd`, so `understand` enforces it again.
- A plan is used once. A live `understand` job is never a cache hit (`force`), and `jobs.retry` refuses it: a new call needs a new plan and a new yes.
- `jobs.submit` refuses `understand` altogether, so the plan and run pair is the only way to it.
- The key is held in memory, keyed by the job id, under a lock taken before the job is submitted, and the runner removes it as it starts that one child. It is never in the catalogue, an event, a log or an error.

## Consequences

- The amount shown to the user can be $0.0001 above the true bound; the spend is still capped by `understand`'s own check against the approved amount.
- A plan's dry run is a job on the subject and makes a checkpoint with no outputs.
- Plans live in memory: a restarted service has none, and the client asks again.
