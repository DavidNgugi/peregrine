//! The renderer a session plays on.
//!
//! A device is what discovery found, deduplicated by protocol and stable id: the protocol gives
//! the id its meaning, because a Chromecast UUID and a DLNA UDN are separate namespaces and one
//! device can be reachable through more than one of them.

use std::fmt;

use thiserror::Error;

use crate::capability::Capabilities;

/// The stable identity a discovery protocol gives one renderer.
///
/// The id is the protocol's own identifier — Google Cast's device UUID or a DLNA renderer's
/// UDN — rather than a database row number, because discovery finds a device before anything
/// has stored it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    /// Builds an id from a non-empty value.
    ///
    /// # Errors
    ///
    /// `DeviceIdError::Empty` when the value is empty or only whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, DeviceIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(DeviceIdError::Empty);
        }
        Ok(Self(value))
    }

    /// The id as discovery reported it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a `DeviceId` could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DeviceIdError {
    /// The value was empty, or only whitespace.
    #[error("device id is empty")]
    Empty,
}

/// The discovery protocol a device was found through.
///
/// The spellings match `device.protocol` in `DATA_MODEL` §2.7, so a value can be stored without a
/// second translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceProtocol {
    /// Google Cast, discovered over mDNS as _googlecast._tcp.
    Cast,
    /// DLNA/UPnP AV, discovered over SSDP.
    Dlna,
}

impl DeviceProtocol {
    /// The value device.protocol stores.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cast => "chromecast",
            Self::Dlna => "dlna",
        }
    }
}

impl fmt::Display for DeviceProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One renderer discovery found, with what it says it can play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The protocol's stable id for this device.
    pub id: DeviceId,
    /// The name the device advertises, or a caller's fallback when it advertises none.
    pub name: String,
    /// How the device was discovered.
    pub protocol: DeviceProtocol,
    /// The model name the device advertises, when it does.
    pub model: Option<String>,
    /// What the device says it can render.
    pub capabilities: Capabilities,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_id_is_never_empty() {
        assert_eq!(DeviceId::new(""), Err(DeviceIdError::Empty));
        assert_eq!(DeviceId::new(" \t\n "), Err(DeviceIdError::Empty));
        assert_eq!(
            DeviceId::new("living-room")
                .expect("a non-empty id")
                .as_str(),
            "living-room"
        );
    }

    #[test]
    fn the_protocol_spellings_match_the_stored_ones() {
        assert_eq!(DeviceProtocol::Cast.as_str(), "chromecast");
        assert_eq!(DeviceProtocol::Dlna.as_str(), "dlna");
    }
}
