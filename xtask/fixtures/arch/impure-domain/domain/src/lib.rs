//! A pure crate that is not pure.
pub fn read(path: &str) -> String {
    // This path is forbidden in a pure crate.
    std::fs::read_to_string(path).unwrap_or_default()
}

pub fn now_is() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
}
