# Native agent schema validation — 2026-09-28

This was a one-time pre-production simplification: no production data required
preservation at the time. Historical database compatibility paths were therefore
removed rather than maintained for this transition. This decision does not set
a policy of discarding older data or foregoing migrations in future releases.

In the tested revision, the agent supported one native database format, marked
`5`. Empty databases were initialized directly; incompatible existing databases
were rejected without migration, repair, reset or old-record adoption, before
host-identity updates. For this transition, any incompatible test-data preparation
or cleanup was left to the caller's deployment script; the agent did not erase
existing databases. Future migration decisions remain outside this record's scope.
See the maintained [deployment guide](../deployment.md#resource-update-schema-gate).

## Tested source

The tested implementation is revision `2908561`, following `448bd29`. SHA-256 of
`git diff --binary 448bd29 2908561 -- crates`:

`c1ea7e5e168dd146ce9e14ac80fc152be740d95ce7373df863ae168f2fa4a87e`

Both authorized hosts used the same locally built release `tt-agent`, SHA-256:

`46df1df43928c0874f3f5b0f276bec069e2773d537d1058eec897274e85d5f4c`

## Checks and results

Local checks passed 179 workspace tests (CLI 19, agent 40, controller 49, core 71),
formatting, Clippy for all targets with warnings denied, workspace checks on the
current compiler and Rust 1.88, and the locked release build. Regressions cover
unchanged incompatible database contents and host identity, rejection of missing
tables/markers, refusal to infer missing bindings, and recovery of healthy rows
with valid native bindings when another VM row is unreadable.

Process-level checks passed on both previously authorized Linux hosts:

- A fresh isolated agent initialized its native database and served authenticated
  host information with an empty inventory.
- A second agent process reopened that database with the same host identity and
  persisted runtime/network bindings.
- Seeded fixtures marked `4` or `6`, or containing a missing/invalid marker,
  failed startup even with an explicit replacement `--host-id`. Database bytes
  remained unchanged.

The probes used private state and runtime directories, temporary loopback ports
and task-only services. Each running agent had a one-CPU, 256 MiB limit with zero
swap; available CPU, memory and disk were checked beforehand. No VM/container
was started, and no existing agent database or deployment was changed. Task
services, credentials and fixture directories were removed; existing SSH, Docker
and containerd service states were preserved. This validates database startup and
reopen behavior, not a new guest lifecycle or production-upgrade run.
