//! The host's battery, for the warning a stream shows when the charge is nearly gone.
//!
//! `starship-battery` is the per-OS reader — `IOKit` on macOS, sysfs on Linux, the Windows power
//! API — so this module is the mapping into this product's vocabulary and nothing else. A host
//! with no battery answers `None`, which the caller draws as "unknown" rather than as "full".

/// The host's charge, as a percentage and whether it is rising.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryState {
    /// Whole percent, 0 to 100.
    pub percent: u8,
    /// Whether the host is charging or full.
    pub charging: bool,
}

/// Reads the first battery the platform reports, when the host has one.
#[must_use]
pub fn state() -> Option<BatteryState> {
    let manager = starship_battery::Manager::new().ok()?;
    let mut batteries = manager.batteries().ok()?;
    let battery = batteries.next()?.ok()?;
    let percent = (battery.state_of_charge().value * 100.0)
        .round()
        .clamp(0.0, 100.0);
    Some(BatteryState {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        percent: percent as u8,
        charging: matches!(
            battery.state(),
            starship_battery::State::Charging | starship_battery::State::Full
        ),
    })
}
