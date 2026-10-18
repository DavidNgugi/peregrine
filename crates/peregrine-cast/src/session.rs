//! One cast session: the renderer it is on, the title it is showing, and how it moves.
//!
//! The four states are the skeleton the adapters will drive: a session starts connecting, the
//! receiver's first play report makes it playing, pause and resume swap between playing and
//! paused, and a stop ends it. stopped is terminal — a new session is opened rather than
//! restarted — because the transport underneath it has already been torn down.
//!
//! A refused event leaves the session exactly as it was, so a late report from a device that
//! has already been stopped cannot resurrect a session the user ended.

use std::fmt;

use thiserror::Error;

use crate::device::DeviceId;

/// The state of one cast session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastSessionState {
    /// The transport is being established and the receiver has not reported playback yet.
    Connecting,
    /// The receiver is playing the title.
    Playing,
    /// The receiver is paused and can resume.
    Paused,
    /// The session is over. Terminal.
    Stopped,
}

impl CastSessionState {
    /// Every state, in the order the session moves through them.
    pub const ALL: [Self; 4] = [Self::Connecting, Self::Playing, Self::Paused, Self::Stopped];

    /// Whether nothing leaves this state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped)
    }

    /// The word a log or a message uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

impl fmt::Display for CastSessionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Something reported by the receiver or requested by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CastSessionEvent {
    /// The receiver reports that the title is playing.
    Started,
    /// The user paused the receiver.
    Paused,
    /// The user resumed a paused receiver.
    Resumed,
    /// The user stopped the session, or the transport ended it.
    Stopped,
}

impl CastSessionEvent {
    /// The word a rejection uses.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Paused => "paused",
            Self::Resumed => "resumed",
            Self::Stopped => "stopped",
        }
    }
}

impl fmt::Display for CastSessionEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an event did not move a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CastSessionRejected {
    /// The session is stopped, and nothing leaves a stopped session.
    #[error("the session is stopped, so {event} cannot move it")]
    Terminal {
        /// The state the event arrived in.
        state: CastSessionState,
        /// The refused event.
        event: CastSessionEvent,
    },
    /// The event is legal somewhere in the machine but not from this state.
    #[error("a {state} session does not accept {event}")]
    OutOfOrder {
        /// The state the event arrived in.
        state: CastSessionState,
        /// The refused event.
        event: CastSessionEvent,
    },
}

/// One session on one renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastSession {
    device_id: DeviceId,
    title_id: i64,
    state: CastSessionState,
}

impl CastSession {
    /// Opens a session for a title on a device, in connecting.
    #[must_use]
    pub const fn new(device_id: DeviceId, title_id: i64) -> Self {
        Self {
            device_id,
            title_id,
            state: CastSessionState::Connecting,
        }
    }

    /// The device the title is being rendered on.
    #[must_use]
    pub const fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// The library title being rendered.
    #[must_use]
    pub const fn title_id(&self) -> i64 {
        self.title_id
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> CastSessionState {
        self.state
    }

    /// Applies one event, or explains why it cannot move this session.
    ///
    /// # Errors
    ///
    /// `CastSessionRejected::Terminal` for any event after a stop, and
    /// `CastSessionRejected::OutOfOrder` for an event the current live state does not accept.
    pub fn apply(&mut self, event: CastSessionEvent) -> Result<(), CastSessionRejected> {
        match self.state {
            CastSessionState::Stopped => {
                return Err(CastSessionRejected::Terminal {
                    state: self.state,
                    event,
                });
            }
            CastSessionState::Connecting => match event {
                CastSessionEvent::Started => self.state = CastSessionState::Playing,
                CastSessionEvent::Stopped => self.state = CastSessionState::Stopped,
                CastSessionEvent::Paused | CastSessionEvent::Resumed => {
                    return Err(self.out_of_order(event));
                }
            },
            CastSessionState::Playing => match event {
                CastSessionEvent::Paused => self.state = CastSessionState::Paused,
                CastSessionEvent::Stopped => self.state = CastSessionState::Stopped,
                CastSessionEvent::Started | CastSessionEvent::Resumed => {
                    return Err(self.out_of_order(event));
                }
            },
            CastSessionState::Paused => match event {
                CastSessionEvent::Resumed => self.state = CastSessionState::Playing,
                CastSessionEvent::Stopped => self.state = CastSessionState::Stopped,
                CastSessionEvent::Started | CastSessionEvent::Paused => {
                    return Err(self.out_of_order(event));
                }
            },
        }
        Ok(())
    }

    fn out_of_order(&self, event: CastSessionEvent) -> CastSessionRejected {
        CastSessionRejected::OutOfOrder {
            state: self.state,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: [CastSessionEvent; 4] = [
        CastSessionEvent::Started,
        CastSessionEvent::Paused,
        CastSessionEvent::Resumed,
        CastSessionEvent::Stopped,
    ];

    /// Every legal cell of the state × event matrix.
    const LEGAL: &[(CastSessionState, CastSessionEvent, CastSessionState)] = &[
        (
            CastSessionState::Connecting,
            CastSessionEvent::Started,
            CastSessionState::Playing,
        ),
        (
            CastSessionState::Connecting,
            CastSessionEvent::Stopped,
            CastSessionState::Stopped,
        ),
        (
            CastSessionState::Playing,
            CastSessionEvent::Paused,
            CastSessionState::Paused,
        ),
        (
            CastSessionState::Playing,
            CastSessionEvent::Stopped,
            CastSessionState::Stopped,
        ),
        (
            CastSessionState::Paused,
            CastSessionEvent::Resumed,
            CastSessionState::Playing,
        ),
        (
            CastSessionState::Paused,
            CastSessionEvent::Stopped,
            CastSessionState::Stopped,
        ),
    ];

    fn device_id() -> DeviceId {
        DeviceId::new("living-room").expect("the fixture id is not empty")
    }

    fn session_in(state: CastSessionState) -> CastSession {
        let mut session = CastSession::new(device_id(), 42);
        match state {
            CastSessionState::Connecting => {}
            CastSessionState::Playing => session
                .apply(CastSessionEvent::Started)
                .expect("connecting accepts started"),
            CastSessionState::Paused => {
                session
                    .apply(CastSessionEvent::Started)
                    .expect("connecting accepts started");
                session
                    .apply(CastSessionEvent::Paused)
                    .expect("playing accepts paused");
            }
            CastSessionState::Stopped => session
                .apply(CastSessionEvent::Stopped)
                .expect("connecting accepts stopped"),
        }
        session
    }

    fn declared(state: CastSessionState, event: CastSessionEvent) -> Option<CastSessionState> {
        LEGAL
            .iter()
            .find(|(from, seen, _)| *from == state && *seen == event)
            .map(|(_, _, to)| *to)
    }

    #[test]
    fn a_session_starts_connecting_and_keeps_its_device_and_title() {
        let session = CastSession::new(device_id(), 42);
        assert_eq!(session.state(), CastSessionState::Connecting);
        assert_eq!(session.device_id(), &device_id());
        assert_eq!(session.title_id(), 42);
    }

    #[test]
    fn the_happy_path_plays_pauses_resumes_and_stops() {
        let mut session = CastSession::new(device_id(), 42);

        session
            .apply(CastSessionEvent::Started)
            .expect("connecting accepts started");
        assert_eq!(session.state(), CastSessionState::Playing);

        session
            .apply(CastSessionEvent::Paused)
            .expect("playing accepts paused");
        assert_eq!(session.state(), CastSessionState::Paused);

        session
            .apply(CastSessionEvent::Resumed)
            .expect("paused accepts resumed");
        assert_eq!(session.state(), CastSessionState::Playing);

        session
            .apply(CastSessionEvent::Stopped)
            .expect("playing accepts stopped");
        assert_eq!(session.state(), CastSessionState::Stopped);
    }

    #[test]
    fn only_the_declared_cells_of_the_matrix_are_legal() {
        for state in CastSessionState::ALL {
            for event in EVENTS {
                let mut session = session_in(state);
                let before = session.clone();
                match (session.apply(event), declared(state, event)) {
                    (Ok(()), Some(expected)) => {
                        assert_eq!(
                            session.state(),
                            expected,
                            "{state:?} + {event:?} moved to {:?}, not {expected:?}",
                            session.state()
                        );
                    }
                    (Err(rejection), None) => {
                        assert_eq!(
                            session, before,
                            "{state:?} + {event:?} was refused but changed the session"
                        );
                        let expected = if state.is_terminal() {
                            matches!(rejection, CastSessionRejected::Terminal { .. })
                        } else {
                            matches!(rejection, CastSessionRejected::OutOfOrder { .. })
                        };
                        assert!(
                            expected,
                            "{state:?} + {event:?} was refused with the wrong reason: {rejection}"
                        );
                    }
                    (Ok(()), None) => panic!(
                        "{state:?} + {event:?} moved to {:?}, but the table calls it illegal",
                        session.state()
                    ),
                    (Err(rejection), Some(expected)) => panic!(
                        "{state:?} + {event:?} should move to {expected:?}, but was refused:                          {rejection}"
                    ),
                }
            }
        }
    }

    #[test]
    fn a_stopped_session_absorbs_every_event() {
        for event in EVENTS {
            assert_eq!(
                session_in(CastSessionState::Stopped).apply(event),
                Err(CastSessionRejected::Terminal {
                    state: CastSessionState::Stopped,
                    event,
                })
            );
        }
    }

    #[test]
    fn a_rejection_names_the_state_and_the_event_it_refused() {
        assert_eq!(
            session_in(CastSessionState::Playing).apply(CastSessionEvent::Resumed),
            Err(CastSessionRejected::OutOfOrder {
                state: CastSessionState::Playing,
                event: CastSessionEvent::Resumed,
            })
        );
        assert_eq!(
            session_in(CastSessionState::Paused).apply(CastSessionEvent::Paused),
            Err(CastSessionRejected::OutOfOrder {
                state: CastSessionState::Paused,
                event: CastSessionEvent::Paused,
            })
        );
        assert_eq!(
            session_in(CastSessionState::Stopped).apply(CastSessionEvent::Started),
            Err(CastSessionRejected::Terminal {
                state: CastSessionState::Stopped,
                event: CastSessionEvent::Started,
            })
        );
    }

    #[test]
    fn every_state_is_reachable_from_connecting() {
        let mut reached = vec![CastSessionState::Connecting];
        let mut frontier = vec![CastSessionState::Connecting];
        while let Some(state) = frontier.pop() {
            for event in EVENTS {
                let mut session = session_in(state);
                if session.apply(event).is_ok() {
                    let next = session.state();
                    if !reached.contains(&next) {
                        reached.push(next);
                        frontier.push(next);
                    }
                }
            }
        }
        for state in CastSessionState::ALL {
            assert!(reached.contains(&state), "{state:?} is unreachable");
        }
        assert_eq!(reached.len(), CastSessionState::ALL.len());
    }
}
