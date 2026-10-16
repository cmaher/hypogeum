#[path = "cases/worker.rs"]
mod worker;
#[path = "cases/board.rs"]
mod board;
#[path = "cases/recovery_db.rs"]
mod recovery;

#[test]
fn integration_suite() { test_support::run_suite(); }
