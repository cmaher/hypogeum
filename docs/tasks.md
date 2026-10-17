# Typed background tasks

`crates/ext/tasks` (`hypgm-ext-tasks`) provides typed, explicitly named Apalis queues, worker policy,
readiness, bounded shutdown, and upstream Board registration. Apalis owns task
IDs, attempts, scheduling, claims, heartbeats and recovery. Application handlers
belong in their owning services.

Create a `Queue<Args>` from an injected tasks-role pool, explicit name and
`WorkerConfig`. Build a task with `queue.task(args)` and submit it through the
upstream storage API. For each worker, create a `worker::Lifecycle` with explicit
`PgPoolOptions` and `PgConnectOptions`, then register a typed handler with
`queue.worker(&lifecycle)`. Run it with
`lifecycle.run_until(worker.run(), signal, config.shutdown_timeout)`.
The lifecycle owns a separate worker pool; budget its connections alongside the
producer/application pool. Use a fresh lifecycle for every restart.
Keep payloads small; store large bytes separately.

Policy validates concurrency and duration bounds, respects the smaller task and
worker attempt limits, supports upstream retry-after/deferred/abort semantics,
and stops retrying when the worker shuts down.
`worker::Lifecycle::run_until` stops claiming on a signal, drains handlers and
persistence, and shares one deadline between worker drain and pool closure.
It retires returned worker connections during shutdown, drops the worker, and
marks the pool closed before awaiting physical cleanup for the remaining time.
If cleanup exceeds the deadline, it returns `DrainTimeout` and continues cleanup
on the active Tokio runtime. Worker completion or errors without a shutdown
signal start a bounded cleanup window of the same duration. External cancellation
also retires the pool before dropping the worker and schedules final closure.
Keep the runtime alive for background cleanup. Producers and handler application
queries must use their own pool. Normal worker connection returns reuse the session without
additional SQL or round trips.

This lifecycle mitigates an Apalis 1.0 / SQLx 0.9 cancellation defect: dropping a
fetch during `BEGIN` can leave a server transaction open. Shutdown-only physical
connection disposal prevents that session from reentering the pool. Apalis still
owns buffered-claim and abandoned-worker recovery. Always use the lifecycle runner
for queue workers; directly running or restarting the upstream worker bypasses
its cancellation guard. Handlers should tolerate repeated delivery after a
worker dies.

For atomic application writes plus enqueue, use upstream
`apalis_postgres::queries::push_tasks` inside the owning repository transaction.
Provision deliberate grants when a future service needs that capability.

## Board

```sh
cargo make apalis-dashboard
```

The standalone `apalis-dashboard` binary in `cmd/apalis-dashboard` uses the
reusable `crates/ext/tasks` Board router. It starts with no queues or handlers and binds
`127.0.0.1:8082` by default. Configure `HYPOGEUM_TASKS_DATABASE_URL`, `HYPOGEUM_TASKS_BOARD_BIND`
and `HYPOGEUM_TASKS_SHUTDOWN_SECONDS` at the composition root. It requires explicit
migrations and `hypogeum_tasks` credentials. Its readiness function exposes
only the upstream schema contract, without ledger access.

For a composed application, construct `Board::new()`, register typed queues, and
serve its router. Duplicate registrations fail. Submission attempt budgets are
capped per queue. The bundled Board interface and empty API need no queue setup.

Run `cargo make test-db` for integration tests covering typed handlers, retries,
submissions, transactional enqueue and recovery after killing a worker process.
Tests use synthetic effects and SQLx-isolated databases in a suite-owned container.
