# Offline agent v5 conversion — 2026-09-29

The OMM fleet upgrade requires preserving existing Firecracker workspaces while
moving from agent schema v5 to v6. The reviewed baseline is
`5115de80be9ff020b1e6b445dcb596097f517c3d`; the change adds the explicit
`upgrade-agent-v5` example and its tests. The running agent still refuses all
non-native database versions. No automatic daemon migration or schema-marker
bypass was introduced.

## Conversion contract

The converter locks the source state directory, opens the source read-only,
requires the native v5 schema and stable host identity, and decodes every VM
using the current shared model. Unknown engines, identity mismatches and
unexpected backup state are rejected. It publishes a new private database
without replacing an existing path. Every original JSON field and metadata
entry is retained; only stable empty backup state and the schema v6 marker are
added. VM disks, runtime processes, networking and SSH state are not modified.

Installing the result remains a separate operator step with both controller and
agent stopped, consistent metadata backups and retained disk recovery points.
See the [upgrade procedure](../deployment.md#resource-update-schema-gate).

## Evidence

- All 221 workspace tests passed on the Linux build host: CLI 21, agent 54,
  controller 57 and core 89; no ignored test cases. The host provided e2fsprogs.
  An initial command omitted `/usr/sbin` from PATH, so `mkfs.ext4` could not be
  found; rerunning with the installed tool in PATH passed without source changes.
- Four converter tests passed: native model/identity preservation and stable
  backup revisions; incompatible schema/record refusal without output or source
  mutation; active-agent lock refusal; and host-identity mismatch refusal.
- Formatting and all-target Clippy with warnings denied passed. Locked release
  compilation of the conversion example passed. Compilation used eight jobs on
  the designated build host; no stress workload or guest allocation was involved.
- A consistent private copy of the real agent v5 database contained five
  Firecracker VM records, four running and one stopped. Conversion preserved all
  five IDs, every non-backup JSON field and all non-version metadata exactly.
  Each output VM had one unique empty backup revision; SQLite `quick_check`
  returned `ok`. This rehearsal did not install the result or stop live guests.

## Limits

This report establishes the offline conversion and its rehearsal, not a completed
fleet deployment. Controller v4-to-v5 conversion and retained guest behavior must
be checked during rollout. The source format is specifically native agent v5;
older, malformed or custom schemas remain unsupported. A VM metadata conversion
is not a guest-disk migration or a backup of application state. Credentials and
private host inventories are excluded from this report.
