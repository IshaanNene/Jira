# ADR-0001: Each service is a library crate with a thin binary

Status: accepted

**Context.** Fifteen logical services are a locked decision; the brief also says to add deployment units only for operational reasons.

**Decision.** Every service crate exposes `router()`, `grpc()` and `workers()` from `lib.rs`; `main.rs` only wires config, pools and shutdown. A `--role api|worker|all` flag selects what a process runs.

**Consequences.** Ownership boundaries are enforced by crates and separate databases, while packaging (one pod per service, API/worker split) is an operational choice. Integration tests can start a service in-process. Cost: discipline is needed so crates never import each other — only `crates/*` are shared, and CI checks this.
