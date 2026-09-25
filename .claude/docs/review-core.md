# TTstack Review Core

Evidence standard, subsystem map, and registry rules. Apply
`pragmatic-engineering.md`: only findings that remove a concrete failure mode.

## 1. Context

1. Full diff + surrounding functions, callers, and cleanup paths — not the patch alone.
2. Map each changed path via the Subsystem Map; load the named guides.
3. Lifecycle, persistence, networking, engine, or deployment change →
   `lifecycle-patterns.md`.
4. Public API, persisted state, documented default, or supported-scope claim →
   `docs/rest-api.md`, `docs/compatibility.md`, `docs/deployment.md`.
5. Full audit → tracked-file ledger first (`git ls-files`); no size or memory guesses.

### Subsystem Map

One primary row per path. Tests are inline `#[cfg(test)] mod tests` in the same file;
there are no `tests/` or `benches/` directories.

| Subsystem | Files | Guides |
|-----------|-------|--------|
| Shared contracts | `crates/core/src/{api,model,auth,guest_config,command,lib}.rs` | `docs/rest-api.md`, `docs/compatibility.md` |
| Engines | `crates/core/src/engine/{mod,qemu,firecracker,docker}.rs`, `engine/firecracker/sandbox.rs` | `docs/guest-images.md`, `lifecycle-patterns.md` |
| Storage and images | `crates/core/src/storage/{mod,file,zvol}.rs` | `docs/guest-images.md`, `lifecycle-patterns.md` |
| Networking and isolation | `crates/core/src/net.rs`, `crates/core/src/net/isolation.rs` | `docs/guest-images.md`, `docs/deployment.md` |
| Agent runtime | `crates/agent/src/{runtime,handler,config,auth}.rs` | `lifecycle-patterns.md`, `docs/rest-api.md` |
| Environment lifecycle | `crates/ctl/src/handler.rs`, `crates/ctl/src/db.rs` | `lifecycle-patterns.md`, `docs/rest-api.md` |
| Placement and scheduling | `crates/ctl/src/scheduler.rs` | `docs/compatibility.md`, `lifecycle-patterns.md` |
| Dashboard | `crates/ctl/src/web.rs` | `docs/rest-api.md` |
| CLI and credentials | `crates/cli/src/{main,client}.rs` | `docs/rest-api.md`, `docs/deployment.md` |
| Deployment and recipes | `crates/cli/src/{deploy,image_builder}.rs`, `tools/deploy.toml.example` | `docs/deployment.md` |
| Build and CI | `Cargo.toml`, `.github/workflows/ci.yml`, `Makefile` | `docs/compatibility.md` |

Documentation, `.claude/`, and dated reports map to the behavior they describe.

## 2. Review depth (effort, not severity)

| Class | Examples | Depth |
|-------|----------|-------|
| Lifecycle / partial failure | create/start/stop/delete, retry, recovery, cleanup | deepest |
| Persisted state / compatibility | schema, stored rows, capability strings, API shapes | deepest |
| Networking / isolation | TAP, bridge, NAT, ownership, jailer, limits | deep |
| Engines / storage | command construction, image sizing, clone, mount | deep |
| Placement / resources | reservation accounting, fit checks, stale snapshots | deep |
| Errors | propagation, partial results, operator message | standard |
| CLI / deployment | defaults, credentials, generated units | standard |
| Docs / tests | alignment with behavior | light unless wrong |

## 3. Evidence

1. Name the invariant (mapped guide).
2. Realistic trigger: timeout, retry, concurrent request, restart or crash, host
   offline, engine absent, corrupt or missing state row, repeated create.
3. Trace callers, cleanup paths, and the controller ↔ agent boundary. A retry is not a
   fresh request.
4. Outcome: orphaned VM, disk, TAP, or port; leaked reservation; stuck or wrong state;
   destruction of the wrong resource; authorization gap; credential in output;
   quantified operator cost.
5. Smallest regression test that fails before the fix.

**Boundaries:** first and last host, host at its configured limits, zero VMs, missing
agent row, empty or oversized port list, absent image, absent engine command,
unreadable persisted row, second controller on the same state directory.

**Authorization:** the API key is a shared administrator credential. An owner label in
a request is not authorization, and an environment is not a tenant.

**Readiness:** a live VMM process is not a booted guest, and a booted guest is not a
ready application. State which one was observed.

## 4. Deterministic / style

fmt / compile / clippy → tools, not findings. Still LOW if tools miss: documentation
contradicting the code, a stale default in a guide, or a link that no longer resolves.

## 5. Audit registry (`docs/audit.md`) — SSOT

Every skill that writes the registry uses these rules and this shape. State meanings:
`workflow-policy.md` §5. Decide from current code, not prior entry text.

1. Prune Open entries proven fixed or obsolete. Narrow scope: in-scope entries only;
   unrelated Open stays unless proven fixed. History → Git and the dated reports. The
   dated disposition that opens the file is its initial record and is never
   re-adjudicated or rewritten by a later review.
2. Add confirmed findings to Open; dedupe by root cause; order CRITICAL → LOW.
3. Re-check Won't Fix entries whose code, callers, or assumptions intersect the scope;
   a full audit re-checks all of them. Prior refutations do not override new evidence.
4. Real but safe fix disproportionate → Won't Fix + Reason. A material claim disproven
   → remove the entry; keep the refutation in the review output or commit history.
   Routine noise → no entry.
5. No dates, freshness markers, or "last reviewed" on entries. Separate entries with `---`.

```markdown
## Open
### [SEVERITY] subsystem: summary
- **Where**: `path` (`fn`) — add lines only when the function is ambiguous
- **What**: defect + realistic trigger (timeout, retry, restart, concurrency, old state)
- **Why**: observable outcome; invariant; why the existing guards fail
- **Suggested fix**: direction + regression test + migration or guide impact

## Won't Fix
### [SEVERITY] subsystem: summary
- **Where** / **What** / **Reason**
```

Severity:

- **CRITICAL**: guest, disk, or state loss; destruction of the wrong resource; silent
  persisted misread; credential exposure; authorization bypass.
- **HIGH**: wrong lifecycle outcome; resources orphaned or leaked on a realistic path;
  state that needs manual repair; destructive behavior on retry.
- **MEDIUM**: edge-case bug; error-policy gap; bounded leak; misleading operator output.
- **LOW**: convention or documentation with a real cost.

Observations are not Open entries.

## Quality gate

Concrete trigger + outcome only. Refute via `false-positive-guide.md`. Agent agreement
and restated intent are not proof.
