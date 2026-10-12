//! The fixture's adapter crate: the one crate its allowlist permits to read a
//! wall clock or a random source.

pub fn now() -> jiff::Timestamp {
    jiff::Timestamp::now()
}

pub fn fresh_id() -> ulid::Ulid {
    ulid::Ulid::generate()
}
