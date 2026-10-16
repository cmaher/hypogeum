# Typed background tasks

`crates/tasks` provides typed, explicitly named Apalis queues, worker policy,
readiness, bounded shutdown, and upstream Board registration. Apalis owns task
IDs, attempts, scheduling, claims, heartbeats and recovery. Application handlers
belong in their owning services.

Create a `Queue<Args>` from an injected tasks-role pool, explicit name and
`WorkerConfig`. Build a task with `queue.task(args)`, submit it through the
upstream storage API, and register a typed handler with `queue.worker(&context)`.
Keep payloads small; store large bytes separately.

Policy validates concurrency and duration bounds, respects the smaller task and
worker attempt limits, supports upstream retry-after/deferred/abort semantics,
and stops retrying when the worker shuts down. `worker::run_until` stops claiming
on a signal, drains persistence, and bounds the drain deadline. Handlers should
tolerate repeated delivery after a worker dies.

For atomic application writes plus enqueue, use upstream
`apalis_postgres::queries::push_tasks` inside the owning repository transaction.
Provision deliberate grants when a future service needs that capability.

## Board

```sh
cargo make tasks-board
```

The standalone binary starts with no queues or handlers and binds
`127.0.0.1:8082` by default. Configure `TASKS_DATABASE_URL`, `TASKS_BOARD_BIND`
and `TASKS_SHUTDOWN_SECONDS` at the composition root. It requires explicit
migrations and `foundation_tasks` credentials. Its readiness function exposes
only the upstream schema contract, without ledger access.

For a composed application, construct `Board::new()`, register typed queues, and
serve its router. Duplicate registrations fail. Submission attempt budgets are
capped per queue. The bundled Board interface and empty API need no queue setup.

Run `cargo make test-db` for integration tests covering typed handlers, retries,
submissions, transactional enqueue and recovery after killing a worker process.
Tests use synthetic effects and SQLx-isolated databases in a suite-owned container.
