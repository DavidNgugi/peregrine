//! The generated TypeScript, beyond what `specta` writes by itself.
//!
//! `specta` exports the command surface and the DTOs from the Rust types. The event
//! topics are not types it can see — they are a manifest in `peregrine-app`, which this
//! crate may name but cannot add a `Type` impl to — so they are appended to the generated
//! file by [`topics_block`]. That keeps one declaration of every topic name and the drift
//! gate still compares the committed file byte for byte.

use std::fmt::Write as _;

use peregrine_app::events::{TOPICS, Topic};

/// The TypeScript declaration of every event topic.
///
/// The block is a `const` object plus the union of its names, so a call site writes
/// `eventTopics.jobProgress.name` and the compiler rejects a topic that does not exist.
#[must_use]
pub fn topics_block() -> String {
    let mut out = String::from(
        "\n/* Event topics: generated from the manifest in `peregrine-app`. */\n\
         /** Every event topic the backend emits, with its payload schema version. */\n\
         export const eventTopics = {\n",
    );
    for topic in TOPICS {
        let _ = writeln!(
            out,
            "\t{}: {{ name: \"{}\", version: {} }},",
            key_for(*topic),
            topic.name(),
            topic.version()
        );
    }
    out.push_str(
        "} as const;\n\n\
         /** The wire name of an event topic. */\n\
         export type EventTopicName = (typeof eventTopics)[keyof typeof eventTopics][\"name\"];\n",
    );
    out
}

/// The TypeScript property name for a topic.
///
/// `job://progress` becomes `jobProgress` and `notification://raised` becomes
/// `notificationRaised`: the namespace stays as the prefix and each `_`-separated part of
/// the event name is capitalised.
#[must_use]
fn key_for(topic: Topic) -> String {
    let Some((namespace, event)) = topic.name().split_once("://") else {
        // `Topic::new` refuses a name with no separator, so this is unreachable; returning
        // the wire name keeps the generator total rather than panicking mid-write.
        return topic.name().to_owned();
    };
    let mut key = String::with_capacity(topic.name().len());
    key.push_str(namespace);
    for part in event.split('_') {
        // An empty part would come from a trailing or doubled underscore, which
        // `Topic::new`'s predicate rejects; skipping keeps the key well-formed.
        let Some(first) = part.chars().next() else {
            continue;
        };
        key.extend(first.to_uppercase());
        key.push_str(&part[first.len_utf8()..]);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every topic in the manifest must appear in the generated block, once.
    #[test]
    fn the_block_declares_every_topic() {
        let block = topics_block();
        for topic in TOPICS {
            let declaration = format!(
                "{}: {{ name: \"{}\", version: {} }}",
                key_for(*topic),
                topic.name(),
                topic.version()
            );
            assert_eq!(
                block.matches(&declaration).count(),
                1,
                "{declaration} must appear exactly once in:\n{block}"
            );
        }
        assert!(block.contains("export type EventTopicName"));
    }

    /// The generator is deterministic, or the drift gate would fail on an unchanged tree.
    #[test]
    fn the_block_is_stable() {
        assert_eq!(topics_block(), topics_block());
    }

    /// The key mapping, including the shape the manifest does not use yet.
    #[test]
    fn keys_are_camel_cased_from_the_wire_name() {
        for (name, expected) in [
            ("job://progress", "jobProgress"),
            ("job://state", "jobState"),
            ("notification://raised", "notificationRaised"),
            ("library://changed", "libraryChanged"),
        ] {
            assert_eq!(key_for(Topic::new(name, 1)), expected);
        }
    }

    /// The keys must be unique, or one topic would silently overwrite another in the
    /// generated object.
    #[test]
    fn keys_are_unique_across_the_manifest() {
        let mut keys: Vec<String> = TOPICS.iter().copied().map(key_for).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "two topics share a property name");
    }
}
