---
description: Review a TTstack diff for concrete lifecycle, correctness, and usability problems.
argument-hint: "[revision range or paths]"
---

Review $ARGUMENTS. If no scope is supplied, review the uncommitted task changes,
including staged changes and relevant untracked files. If there are no changes,
report that instead of guessing a whole-repository audit scope.

Read [AGENTS.md](../../AGENTS.md) and use the boundaries in the
[development skill](../skills/ttstack-development/SKILL.md). Read callers and
cleanup paths, not just the patch. Focus on concrete partial-failure, retry,
stop/start, deletion, resource accounting, authorization, and operator-experience
problems. Linux is primary; FreeBSD is experimental.

Report actionable findings with file/line evidence, the triggering condition,
impact, and the smallest practical correction. Separate observed defects from
unverified concerns. A clean review still states the checked scope and limits.
Review alone does not authorize fixes, commits, deployment, or live host tests;
follow any additional authorization already present in the user's task.
