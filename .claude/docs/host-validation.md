# Authorized Host Validation

Policy for `/x-live` and any bounded functional test on a real host. A pass here is
not a security certification or a capacity measurement.

## Authorization and bounds

- Use only hosts and operations already authorized in the conversation. With none
  authorized, prepare the local artifacts and a concrete plan before asking for access.
- Inspect load, free memory and disk, existing services, and engine prerequisites first.
- On the authorized test machines, budget roughly half of host CPU and memory,
  accounting for existing workloads and leaving the other half as headroom. No stress
  or throughput testing.
- If capacity is insufficient, reduce concurrency or use another authorized host.

## Isolation

- Distinct environment IDs, ports, runtime directories, and controller/agent state
  directories per run.
- Prefer locally built artifacts and a separate test setup; keep a list of the resources
  created.
- Never replace an existing service, reset shared firewall rules, or stop another guest.
  Do not probe unrelated machines.
- Never hard-code operator credentials or private host inventories in a workflow,
  script, report, or command line. Keys and administrative material stay out of logs,
  output, and SSH argument lists.

## Cases worth running

Choose from the actual change; do not run the whole list blindly.

- Create, observe the assigned endpoints, and wait for guest and application readiness.
- Stop/start and verify that a written marker survives; do not expect memory to survive.
- Retry a timed-out operation by inspecting its existing state first.
- Firecracker configuration: verify expected contents and read-only access without
  putting secrets in output; configuration is immutable across boots.
- Isolation: use controlled endpoints and distinguish host/peer denial from allowed
  egress and replies to published ports.
- Lifecycle recovery: restart only the isolated test services and inspect persisted
  state, processes, networking, and reservations.
- Delete test environments and verify that their resources are released.

Normal shutdown and forced termination are different results; record which was observed.
A failed probe can be a guest, routing, firewall, or readiness problem — collect bounded
evidence before changing the implementation.

## Evidence and cleanup

- Clean up task-owned guests, services, namespaces, tunnels, rules, and files, including
  after failures; verify that pre-existing services still hold their expected state;
  report cleanup that could not be completed.
- When retaining evidence, write a dated report under `docs/validation/`, link it from
  `docs/README.md`, and state the tested revision, resource bounds, cases, observations,
  cleanup, and untested scope. Omit keys and private configuration.
- Feed reproducible defects back into a focused change, then rerun only the affected checks.
