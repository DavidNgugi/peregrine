//! The log-scanning test for `PG-E24-05`.
//!
//! Every test here writes through the same shape of subscriber the application
//! installs — JSON, redacting, into a file — and then reads back every file that
//! was produced. The acceptance is "a log-scanning test finds no key material
//! across all log paths", so the assertions are on the bytes on disk, and each
//! positive `contains` is a negative control that keeps the scan from passing
//! because it read nothing.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use peregrine_secrets::{RedactingFormat, Secret};
use tracing::Subscriber;
use tracing_subscriber::Registry;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt as _;

/// The subscriber the application installs, writing into `path`.
fn subscriber_over(path: &Path) -> impl Subscriber + Send + Sync + 'static {
    let file = fs::File::create(path).expect("the log file must be creatable");
    Registry::default().with(
        fmt::layer()
            .event_format(RedactingFormat::new(fmt::format::Format::default().json()))
            .with_writer(Mutex::new(file)),
    )
}

/// Every byte the subscriber produced, across every file under `directory`.
fn read_all(directory: &Path) -> String {
    let mut text = String::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                // Lossy rather than `read_to_string`: a log file with a byte that
                // is not UTF-8 must still be scanned.
                text.push_str(&String::from_utf8_lossy(&fs::read(&path).unwrap()));
            }
        }
    }
    text
}

/// Runs `events` against a fresh log file and returns what was written.
fn emitted(directory: &Path, events: impl FnOnce()) -> String {
    let subscriber = subscriber_over(&directory.join("peregrine.log"));
    {
        let _default = tracing::subscriber::set_default(subscriber);
        events();
    }
    let text = read_all(directory);
    assert!(
        !text.is_empty(),
        "the subscriber wrote nothing, so no scan of it proves anything"
    );
    text
}

#[test]
fn the_debug_form_of_a_secret_is_a_fixed_marker() {
    let secret = Secret::new("hunter2-must-not-appear");

    let rendered = format!("{secret:?}");

    assert_eq!(rendered, "Secret([REDACTED])");
    assert_eq!(secret.expose_secret(), "hunter2-must-not-appear");
}

#[test]
fn no_key_material_reaches_the_log_files() {
    const SECRET: &str = "ghp_A1b2C3d4E5f6G7h8I9j0KlMnOpQrStUvWxYz";
    const UNREGISTERED: &str = "plugin-token-never-registered-9f8e7d6c";

    let secret = Secret::new(SECRET);
    let directory = tempfile::tempdir().unwrap();

    let logs = emitted(directory.path(), || {
        // The mistake this filter exists to survive: the value is exposed into
        // an event, as a field and inside the message.
        tracing::info!(token = %secret.expose_secret(), "at the provider boundary");
        tracing::warn!("bearer {}", secret.expose_secret());
        tracing::debug!(credentials = ?secret, "debug form");
        tracing::info!(token = UNREGISTERED, "the unregistered control");
    });

    assert!(
        !logs.contains(SECRET),
        "the registered secret reached the log: {logs}"
    );
    assert!(
        !logs.contains("A1b2C3d4"),
        "a prefix of the registered secret reached the log: {logs}"
    );
    assert!(logs.contains("[REDACTED]"), "the marker is missing: {logs}");
    // The negative control: the same scan finds a value that was never
    // registered, so a green run cannot be a scan of nothing.
    assert!(
        logs.contains(UNREGISTERED),
        "the scan did not find the unregistered control: {logs}"
    );
}

#[test]
fn a_secret_is_removed_in_both_its_raw_and_json_escaped_forms() {
    // The JSON formatter escapes these, so a raw-only needle would miss the
    // value on the way to the file.
    const SECRET: &str = "pw\"with\\escapes\nand-a-tab\tend";
    const UNREGISTERED: &str = "public\"value";

    let secret = Secret::new(SECRET);
    let directory = tempfile::tempdir().unwrap();

    let logs = emitted(directory.path(), || {
        tracing::info!(password = %secret.expose_secret(), "escaped secret");
        tracing::info!(public = UNREGISTERED, "escaped control");
    });

    assert!(
        !logs.contains("escapes"),
        "the escaped secret reached the log: {logs}"
    );
    assert!(
        !logs.contains("and-a-tab"),
        "part of the secret reached the log: {logs}"
    );
    assert!(logs.contains("[REDACTED]"), "the marker is missing: {logs}");
    // The control proves the scan sees escaped output at all.
    assert!(
        logs.contains(r#"public\"value"#),
        "the scan did not find the escaped control: {logs}"
    );
}

#[test]
fn a_registered_prefix_does_not_leave_the_tail_of_a_longer_secret() {
    const PREFIX: &str = "token-TOP-SECRET";
    const LONGER: &str = "token-TOP-SECRET-ALPHA-BRAVO";

    let shorter = Secret::new(PREFIX);
    let longer = Secret::new(LONGER);
    let directory = tempfile::tempdir().unwrap();

    let logs = emitted(directory.path(), || {
        tracing::info!(token = %longer.expose_secret(), "the longer secret");
        tracing::info!(token = %shorter.expose_secret(), "the prefix alone");
    });

    assert!(
        !logs.contains("ALPHA-BRAVO"),
        "the tail of the longer secret survived: {logs}"
    );
    assert!(!logs.contains("TOP-SECRET"), "a secret survived: {logs}");
}

#[test]
fn an_empty_secret_does_not_erase_the_line() {
    // An empty needle matches at every position, so registering one would
    // replace the whole line with the marker.
    let _empty = Secret::new("");
    let directory = tempfile::tempdir().unwrap();

    let logs = emitted(directory.path(), || {
        tracing::info!("the line survives: keeper-marker");
    });

    assert!(logs.contains("the line survives"), "{logs}");
    assert!(logs.contains("keeper-marker"), "{logs}");
}
