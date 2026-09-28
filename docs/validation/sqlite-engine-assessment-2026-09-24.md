# SQLite engine assessment — 2026-09-24

This archived assessment was originally recorded in the compatibility guide. It
reports the dependency-refresh decision and local probes from that date; the
original note did not identify an exact source revision. It has not been rerun
as part of the documentation update. Current behavior is described in the
[compatibility guide](../compatibility.md).

TTstack retains upstream SQLite through bundled `rusqlite`. The dependency refresh
updates all 12 direct registry dependencies to the latest non-yanked stable releases
and refreshes the lockfile within upstream constraints. In particular, Axum 0.8.9
requires exactly `matchit = 0.8.4`; forcing a newer release would require changing
that upstream dependency. `Cargo.toml` and `Cargo.lock` record the selected versions.

[Turso 0.7.2](https://github.com/tursodatabase/turso/tree/v0.7.2) is a Rust SQLite
reimplementation with reported production users, so it is a credible candidate.
Its release documentation still marks multi-process WAL as experimental and excludes
mixed SQLite/Turso multi-process access from its
[compatibility guarantees](https://github.com/tursodatabase/turso/blob/v0.7.2/COMPAT.md).
[prsqlite](https://github.com/kawasin73/prsqlite) describes itself as an unstable
work-in-progress. [libSQL](https://github.com/tursodatabase/libsql) is a SQLite fork,
not a pure Rust replacement for its engine.

An isolated local probe of Turso 0.7.2's Rust API passed independent-reader isolation,
dropped-transaction rollback and write rejection with `query_only=1`. It also found
that `execute_batch("PRAGMA journal_mode=WAL")` returns an unexpected-row error and
`PRAGMA query_only=ON` is rejected while `query_only=1` works. These are API/SQL
compatibility differences, not evidence of data loss; a passing smoke test is also
not a durability certification. The engine can be used with adaptations, but our
assessment is that switching TTstack's recovery-critical state store now adds more
compatibility and operational work than it removes. No alternate database backend
or migration path is introduced by this dependency refresh.

Validation of the retained backend passed 118 workspace tests, Clippy, Rust 1.88
checks and a locked release build. A local process-level upgrade check used the
previous controller binary to create state, killed it with an outstanding WAL,
then verified recovery, authenticated API/CLI access, writes and another restart
with the new binary. SQLite integrity checking and reopening with the previous
binary also passed. This check did not boot guests or exercise the remote hosts.
