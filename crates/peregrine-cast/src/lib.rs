#![forbid(unsafe_code)]
//! Discovery, the renderer capability model, the Chromecast CASTV2 and DLNA
//! `AVTransport` adapters, delivery-mode decisions, and playback-state reconciliation.
//!
//! Off by default, and never scans beyond the standard discovery protocols.
//!
//! What exists today is the first slice: the device and capability model with its matching rule,
//! the discovery port with a deterministic fake used by its tests, and the cast session state
//! machine. The mDNS and SSDP transports, the CASTV2 and SOAP adapters, and the delivery choice
//! that turns a capability gap into a remux, a transcode, or a burn-in are the next slices
//! (PG-E19-01, PG-E19-05, PG-E19-08, PG-E19-09).

mod capability;
mod device;
mod discovery;
mod session;

pub use capability::{Capabilities, CapabilityGap, Container, MediaRequirements};
pub use device::{Device, DeviceId, DeviceIdError, DeviceProtocol};
pub use discovery::{DeviceDiscovery, DiscoveryError, DiscoveryFuture};
pub use session::{CastSession, CastSessionEvent, CastSessionRejected, CastSessionState};
