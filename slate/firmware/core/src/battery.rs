//! Battery bands from the cell voltage.
//!
//! Slate has no fuel gauge. A 3.3 MΩ / 1.0 MΩ divider feeds the ADC and the firmware maps
//! the cell voltage onto coarse bands. The server, which renders every frame, turns the
//! band it receives in each check into on-screen state; the device itself never draws.
//! Thresholds are provisional until measured against the chosen pouch's discharge curve.

/// Coarse battery state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    /// Normal operation.
    Ok,
    /// Server should show a charge-soon badge; behaviour is otherwise unchanged.
    Low,
    /// No more waveforms. The last image stays up; the device checks rarely and only reports.
    Critical,
    /// Below the safe floor: enter deep sleep until USB power returns.
    Exhausted,
}

/// Below this, report [`Band::Low`].
pub const LOW_MV: u16 = 3_600;
/// Below this, report [`Band::Critical`] and stop refreshing.
pub const CRITICAL_MV: u16 = 3_450;
/// Below this, [`Band::Exhausted`]: stop the radio and deep sleep.
pub const EXHAUSTED_MV: u16 = 3_300;
/// Readings must move this far past a threshold before the band changes back.
pub const HYSTERESIS_MV: u16 = 50;

const _: () = assert!(LOW_MV > CRITICAL_MV && CRITICAL_MV > EXHAUSTED_MV);
