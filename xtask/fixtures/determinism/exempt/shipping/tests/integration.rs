//! An integration test tree, which never ships.

#[test]
fn an_integration_test_may_read_the_wall_clock() {
    let _ = jiff::Timestamp::now();
}
