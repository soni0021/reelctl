# Delegation to subagents

Keep the main context small. Hand broad, mechanical work to the Haiku 5.5 subagents in `.claude/agents/`, and keep work that needs exact output in the main session.

- **search**: questions that sweep many files or directories, where only the conclusion matters. `Explore` does the same job: `.claude/agents/explore.md` replaces the built-in one so that plan mode's searches also run on Haiku 5.5.
- **grep**: every definition, caller, and mention of a symbol, field, key, or string before changing it.
- **audit**: after a task's commit, before declaring the task done or merging. It sees only committed work, so commit first. The main session still runs `cargo test` once before that commit, as AGENTS.md says, because the commit records the result.

Do these in the main session instead:

- Reading or grepping one file whose path is already known.
- TDD red and green runs, and `cargo check` on edit. They run on uncommitted code and need the exact output.

Consult the advisor before a spec or plan is presented for approval, when the same error appears a second time, and after the audit, before declaring a task done.

When delegating, give the subagent the question, the paths and symbols already known, and what to return. Its answer is not shown to the user, so relay what matters.
