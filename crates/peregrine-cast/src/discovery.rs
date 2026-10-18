//! The device discovery port.
//!
//! The port is object-safe and returns boxed futures because the application will hold one
//! adapter behind Arc<dyn DeviceDiscovery>, chosen at startup. A concrete adapter carries its own
//! transport — mDNS and SSDP in the next slice (PG-E19-01) — and never leaks a third-party
//! discovery type through this interface (AGENTS R8).

use std::{future::Future, pin::Pin};

use thiserror::Error;

use crate::device::Device;

/// The future a discovery call returns.
pub type DiscoveryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Device>, DiscoveryError>> + Send + 'a>>;

/// Why discovery could not produce a device list.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DiscoveryError {
    /// The transport could not start — a socket could not be bound, or the protocol is
    /// unavailable on this platform.
    #[error("device discovery is unavailable: {reason}")]
    Unavailable {
        /// What refused to start, for the log. Never a path or a secret.
        reason: String,
    },
}

/// The adapter seam discovery sits behind.
pub trait DeviceDiscovery: Send + Sync {
    /// Runs a full discovery pass and answers every device this adapter can currently see.
    ///
    /// An empty list is a successful answer, not an error: a network with no renderer on it is
    /// an ordinary state.
    fn discover(&self) -> DiscoveryFuture<'_>;

    /// Re-queries the devices already known and answers the current list.
    ///
    /// A device that is gone is absent from the answer rather than reported as an error, so a
    /// caller can replace its list with the result.
    fn refresh(&self) -> DiscoveryFuture<'_>;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::capability::Capabilities;
    use crate::device::{DeviceId, DeviceProtocol};

    /// A deterministic adapter: the first pass answers one fixed list, every refresh answers a
    /// second fixed list, and the refresh count is observable so a test can tell the two calls
    /// apart.
    struct FakeDiscovery {
        discovered: Vec<Device>,
        refreshed: Vec<Device>,
        refreshes: AtomicUsize,
    }

    impl FakeDiscovery {
        fn new(discovered: Vec<Device>, refreshed: Vec<Device>) -> Self {
            Self {
                discovered,
                refreshed,
                refreshes: AtomicUsize::new(0),
            }
        }
    }

    impl DeviceDiscovery for FakeDiscovery {
        fn discover(&self) -> DiscoveryFuture<'_> {
            let devices = self.discovered.clone();
            Box::pin(async move { Ok(devices) })
        }

        fn refresh(&self) -> DiscoveryFuture<'_> {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
            let devices = self.refreshed.clone();
            Box::pin(async move { Ok(devices) })
        }
    }

    fn device(id: &str, name: &str, protocol: DeviceProtocol) -> Device {
        Device {
            id: DeviceId::new(id).expect("the fixture id is not empty"),
            name: name.to_owned(),
            protocol,
            model: None,
            capabilities: Capabilities::default(),
        }
    }

    #[tokio::test]
    async fn discovery_answers_its_deterministic_list_every_time() {
        let first = device("cast-1", "Living Room", DeviceProtocol::Cast);
        let second = device("dlna-1", "Bedroom TV", DeviceProtocol::Dlna);
        let fake = FakeDiscovery::new(vec![first.clone(), second.clone()], vec![first.clone()]);

        assert_eq!(
            fake.discover().await.expect("the fake never fails"),
            vec![first.clone(), second.clone()]
        );
        assert_eq!(
            fake.discover().await.expect("the fake never fails"),
            vec![first.clone(), second]
        );
        assert_eq!(fake.refreshes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refresh_answers_the_current_list_and_is_counted() {
        let first = device("cast-1", "Living Room", DeviceProtocol::Cast);
        let fake = FakeDiscovery::new(vec![first.clone()], vec![first.clone()]);

        assert_eq!(
            fake.refresh().await.expect("the fake never fails"),
            vec![first.clone()]
        );
        assert_eq!(
            fake.refresh().await.expect("the fake never fails"),
            vec![first]
        );
        assert_eq!(
            fake.refreshes.load(Ordering::SeqCst),
            2,
            "each refresh is one call"
        );
    }
}
