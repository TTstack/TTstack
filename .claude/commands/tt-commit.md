---
description: Validate and commit the requested TTstack change without including unrelated work.
argument-hint: "[scope or commit summary]"
---

Prepare a commit for $ARGUMENTS, or the current task when omitted.
Read [AGENTS.md](../../AGENTS.md). Inspect the branch, staged and unstaged changes,
and untracked files. Reuse relevant checks already completed in this session;
run missing checks using [ttstack-development](../skills/ttstack-development/SKILL.md).

Stage only the intended files or hunks, review the staged diff, and run
`git diff --cached --check`. Preserve unrelated staged work; if it prevents a
clean scoped commit, resolve that boundary before committing. Use an English
conventional commit message and the author requested by the user, otherwise the
repository's configured identity. Do not invent an author or amend earlier work.

Create the local commit. Push when the user has requested it in the task or
session, using the intended remote branch without force-pushing; otherwise stop
at the local commit. Report the hash, checks, and publication state. Invocation
does not authorize deployment or unrelated repository changes.
