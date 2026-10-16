#[path = "cases/begin_barrier.rs"]
mod begin_barrier;
#[path = "cases/board.rs"]
mod board;
#[path = "cases/cleanup.rs"]
mod cleanup;
#[path = "cases/recovery_db.rs"]
mod recovery;
#[path = "cases/worker.rs"]
mod worker;

#[test]
fn integration_suite() {
    test_support::run_suite();
}
