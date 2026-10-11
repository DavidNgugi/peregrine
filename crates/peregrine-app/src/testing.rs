//! Test doubles shared by this crate's unit tests.
//!
//! [`RecordingSink`] is the one that matters: the event bus's only collaborator is its
//! sink, and the tests that prove the delivery rate, the coalescing, and the degraded
//! signal all need the same sink that records what it received.

use std::sync::Mutex;

use crate::events::{Event, EventSink, EventSinkError};

/// A sink that records every event, and can be told to refuse them.
#[derive(Default)]
pub struct RecordingSink {
    events: Mutex<Vec<Event>>,
    refuse: bool,
}

impl RecordingSink {
    /// A sink whose transport always fails, for the degraded-signal tests.
    #[must_use]
    pub fn refusing() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            refuse: true,
        }
    }

    /// Everything delivered so far, in delivery order.
    #[must_use]
    pub fn delivered(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: &Event) -> Result<(), EventSinkError> {
        if self.refuse {
            return Err(EventSinkError::new("the transport is gone"));
        }
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}
