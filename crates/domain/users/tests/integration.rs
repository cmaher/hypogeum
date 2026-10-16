#[path = "cases/repo.rs"]
mod repo;
#[path = "cases/transport.rs"]
mod transport;

#[test]
fn integration_suite() {
    test_support::run_suite();
}
