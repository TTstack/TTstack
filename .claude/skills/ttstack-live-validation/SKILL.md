---
name: ttstack-live-validation
description: Plan and perform low-load TTstack lifecycle, guest configuration, networking, and restart checks on an authorized Linux host. Use for functional VM acceptance, not benchmarking or production deployment.
---

# Lightweight TTstack Live Validation

Read [AGENTS.md](../../../AGENTS.md), the relevant [guest contract](../../../docs/guest-images.md),
and the [Firecracker validation report](../../../docs/firecracker-validation-2026-09-24.md)
for useful cases and the limits of previous evidence. Prior reports are not
permission to reuse a host or repeat every test.

## Bound the experiment

Use hosts and operations already authorized in the conversation. If none are
authorized, prepare the local artifacts and a concrete test plan before requesting
the missing host/access information. Do not seek approval again for a step already
covered by the user's instructions.

Inspect host load, free memory/disk, existing services, and engine prerequisites
before allocating. Size guests for the actual image and functional scenario.
On the authorized test machines, budget roughly half of host CPU and memory,
accounting for existing workloads and leaving the other half as headroom. There
is no need to force an unrealistically tiny guest or a fixed guest count. Use the
concurrency needed to test the flow without turning it into a saturation test.
If capacity is insufficient, reduce concurrency or use another authorized host.

Use distinct environment IDs, ports, runtime directories, and controller/agent
state. Keep a list of created resources. Prefer locally built artifacts, bounded
commands and probes, and a separate test setup. Do not replace existing services,
reset shared firewall rules, or use stress/throughput tests for functional coverage.

## Verify the relevant behavior

Choose cases from the actual change rather than running the entire list blindly:

- Create, observe assigned endpoints, and wait for guest/application readiness.
- Stop/start and verify a written marker survives; do not expect memory to survive.
- Retry a timed-out operation by inspecting its existing state first.
- For Firecracker configuration, verify expected contents and read-only access
  without putting secrets in output; configuration is immutable across boots.
- For isolation, use controlled endpoints and distinguish host/peer denial from
  allowed egress and replies to published ports. Do not probe unrelated machines.
- For lifecycle recovery, restart only the isolated test services and inspect
  persisted state, processes, networking, and resource reservations.
- Delete test environments and verify that their resources are released.

Normal shutdown and forced termination are different results. Record which was
observed. A failed probe can be a guest, routing, firewall, or readiness issue;
collect bounded evidence before changing the implementation.

## Close the loop

Clean up only task-owned guests, services, namespaces, tunnels, rules, and files,
including after failures. Verify the pre-existing services still have their
expected state. Report any cleanup that could not be completed.

When retaining evidence, write a dated report under `docs/` and link it from
[the documentation index](../../../docs/README.md). Include the tested revision,
resource bounds, cases, observations, cleanup, and untested scope. Omit keys and
private configuration. Feed reproducible defects back into a focused code change,
then rerun only the affected checks. Do not present a smoke test as a security
certification, capacity measurement, or validation of an untested backend.
