//! Usenet vocabulary shared by the storage layer and the settings surface.

use crate::stored_enum;

stored_enum! {
    /// A provider's last known health, as `usenet_server.health_state` stores it.
    UsenetHealth {
        /// No connection attempt has been recorded yet.
        Unknown => "unknown",
        /// The last authentication and greeting succeeded.
        Ok => "ok",
        /// The last attempt answered, but slowly or with a recoverable error.
        Degraded => "degraded",
        /// The last attempt failed with an authentication or connection error.
        Failing => "failing",
        /// The user disabled the server, so nothing contacts it.
        Disabled => "disabled",
    }
}
