---
description: Run the local checks appropriate to a TTstack change and report their limits.
argument-hint: "[paths or check scope]"
---

Check $ARGUMENTS, or the current task's uncommitted changes when omitted.
Read [AGENTS.md](../../AGENTS.md) and apply the check selection in
[ttstack-development](../skills/ttstack-development/SKILL.md).

Resolve whether the change is documentation, a focused Rust change, a shared
contract change, or a toolchain/dependency change. Run the relevant checks and
inspect every result. Do not deploy, start guests, silently rewrite dependencies,
or turn a failed check into a broader refactor. Report failures and handle fixes
only within the user's authorized scope. State omitted or skipped checks clearly.
