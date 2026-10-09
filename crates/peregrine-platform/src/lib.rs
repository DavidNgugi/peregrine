#![forbid(unsafe_code)]
//! Per-OS path resolution, permission requests, tray, dock and taskbar badges,
//! autostart, notifications, power-save inhibition, and the OS authentication
//! adapters.
//!
//! One cfg-gated module per OS behind a common interface. An adapter reports which
//! mechanism answered, and reports unavailability rather than faking success.
