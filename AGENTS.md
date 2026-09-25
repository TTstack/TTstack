# Working on TTstack

TTstack is a small, general-purpose VM and container manager. Favor reliable
core lifecycle behavior and a simple operator experience over new abstractions
or optional subsystems. User identity, application installation, and business
policies belong to callers, not to TTstack.

## Scope and ownership

- `crates/core` (`ttcore`): shared API models, engines, storage, and networking.
- `crates/agent` (`tt-agent`): host-local lifecycle and persistent runtime state.
- `crates/ctl` (`tt-ctl`): scheduling, environment lifecycle, fleet state, and UI.
- `crates/cli` (`tt`): user commands, image recipes, and deployment tooling.

Linux x86_64 is the supported host platform. Keep the host implementation focused
on QEMU/KVM, Firecracker, and Docker/Podman. Report unverified combinations clearly.

## Development and verification

Read [README.md](README.md) and the relevant [documentation](docs/README.md).
Use the Rust version and dependency policy in [Cargo.toml](Cargo.toml); the
current minimum is Rust 1.88. Keep `Cargo.lock` consistent with dependency edits.

The [development skill](.claude/skills/ttstack-development/SKILL.md) maps changes
to focused tests and workspace checks. Formatting, Clippy, tests, and the MSRV
gate are defined in [.github/workflows/ci.yml](.github/workflows/ci.yml).
Documentation-only edits need link, example, and diff checks rather than a full
Rust build. Do not introduce tests that merely repeat documentation wording.

Check lifecycle changes for partial failure and retries. A timeout does not prove
that creation failed. Stop retains disks but not VM memory; delete is destructive.
API owner labels are not authorization. VM process readiness is not application
readiness. Shared API changes may require agent/controller capability checks.

Keep repository documentation, instructions, and commit messages in English.
Respond to the user in their language. Preserve unrelated work and use the
current task's authorization for commits, pushes, and remote operations; these
files do not independently authorize deployment or changes to other repositories.

## Live systems

Use the [live-validation skill](.claude/skills/x-live/SKILL.md) when functional VM
testing is needed on an authorized host. Prefer local unit
checks first. Keep remote tests bounded and isolated from existing services.
For the authorized test machines, an approximate ceiling of 50% of host CPU and
memory is a practical functional-test budget, accounting for existing load.
No stress testing is part of the normal workflow.

Never hard-code operator credentials or private host inventories in workflows.
Keep administrative keys out of logs and reports. Track and clean up only the
resources created by the task; do not reset host networking or stop other guests.

## Documentation and workflows

Each behavior has one maintained description, linked from `docs/README.md`.
Dated validation reports describe their tested revision and limitations, not a
promise about every platform. Update examples and incoming links with API or path
changes. Keep Claude entry points small and link to shared instructions rather
than copying them. See the [workflow index](.claude/README.md).
