# False-Positive Guide

Refute every candidate finding against this list before reporting it.
`review-core.md` §3 defines the evidence a finding needs.

## Not defects

- **Documented product boundaries.** One controller; configured guardrails of 50 hosts
  and 1000 tracked VMs; no HA, migration, distributed storage, image distribution, or
  automatic guest restart after a host reboot; Docker lacks managed SSH keys, disk
  quotas, and outgoing restrictions; Firecracker lacks managed SSH keys, interactive
  consoles, and resizing of existing VMs. See `docs/compatibility.md`.
- **Deliberate non-isolation.** A guest can reach a wildcard management listener; the
  deployment and guest guides document this as operator responsibility.
- **Shared administrator credential.** The API key authenticates callers, not servers,
  and a self-reported host id does not establish trust; registration still requires a
  trusted address and protected transport.
- **Caller-owned policy.** User identity, entitlements, application installation,
  connection leases, and idle-stop policy belong to callers, not to TTstack.
- **Recorded coverage gaps.** OpenRC, Podman, QEMU+zvol, musl, and remote deployment
  are implemented without fresh live evidence; `docs/compatibility.md` tracks that. A
  gap becomes a finding only when code or docs contradict their own claim.
- **Dated reports.** A report describes the revision it tested and its limits, not
  current behavior. Age alone is not a defect.
- **Tool output.** fmt, clippy, and compile errors are tools, not review findings.
- **Test environment.** A test skipped for a missing external tool (`e2fsprogs`,
  `qemu`, `docker`) is unverified scope, not a failing test.
- **Style and preference.** Naming, ordering, and formatting preferences without a
  concrete failure are not findings.
- **Restated intent.** Code that is correct but could be written differently is not a
  finding, and neither is a comment that accurately describes the code.
- **Unquantified performance.** A hot-path concern needs a measured or bounded cost,
  not a suspicion.
