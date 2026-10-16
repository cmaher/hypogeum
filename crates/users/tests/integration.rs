#[path = "cases/repo.rs"]
mod repo;

#[test]
fn integration_suite() {
    test_support::run_suite();
}
