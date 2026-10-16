# Apalis persistence shutdown patch

`apalis-core/` is the crates.io `apalis-core` 1.0.0 package (MIT), selected by
`[patch.crates-io]` in the root manifest. It is excluded from workspace membership.
Published package SHA-256: `93a749c6487cede1a1ca503997f59fca75aeca305fcbf555d5a13606b1c7ecb1`.
The package-local lockfile and Cargo vendor checksum file are omitted; the
workspace lockfile resolves this path patch. The upstream license is included.

Only `src/backend/persistence/mod.rs` differs from the published Rust source:
`Persisted::poll_close` finishes `State::Fetch` and moves its results into
`State::Buffered`. The existing shutdown path then requeues those claims.
Dropping a pending fetch could cancel SQLx 0.9.0's `BEGIN` before it records the
transaction depth, returning a connection with an open transaction to the pool.
This caused the intermittent retry integration timeout investigated in fnd-50sct.

The regression in `crates/tasks/tests/cases/worker.rs` stops a real PostgreSQL
backend after `BEGIN` reaches the server but before its reply is read. It checks
both empty and nonempty fetches: no transaction leaks and no stranded claims.
It runs through the ordinary suite-owned Testcontainers launcher.

Remove the patch and this directory when upgrading to an upstream release that
finishes pending fetches and releases their claims during shutdown. Run the
regression and `cargo make ci` against that release before removing the patch.
The existing drain deadline still applies; a forced deadline can cancel work.

Upstream: https://github.com/apalis-dev/apalis
License: https://github.com/apalis-dev/apalis/blob/master/LICENSE
