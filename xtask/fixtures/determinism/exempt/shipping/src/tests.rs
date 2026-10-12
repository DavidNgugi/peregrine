//! A test module in its own file: it is compiled only under `cfg(test)`, so the
//! gate must skip it even though the file carries no attribute of its own.

#[test]
fn the_file_is_test_only() {
    let _ = jiff::Timestamp::now();
}
