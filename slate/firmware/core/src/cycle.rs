//! The update cycle: idle, check, fetch, refresh, power down.
//!
//! One cycle, in order (`docs/firmware.md` > "Update cycle"):
//!
//! 1. Wake from light sleep when the server-chosen check interval expires.
//! 2. `POST s1/check` with the shown version and telemetry.
//! 3. On `NoChange`, sleep again. On `NewFrame`, enable the panel rail, initialise the
//!    controller, switch Thread to rx-on-when-idle, and fetch the frame in blocks.
//! 4. Decode each row straight into panel RAM; verify length and CRC-32.
//! 5. Only a verified frame starts a waveform. Light-sleep until BUSY clears.
//! 6. Send the controller to deep sleep, drive every panel line low, open the load switch,
//!    return Thread to sleepy-end-device polling.
//!
//! While USB power is present the device never sleeps, so the console and flashing work.

use slate_protocol::frame::Waveform;

/// Where the device is in its cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// No Thread dataset or server address stored; waiting on the USB console.
    Unprovisioned,
    /// Joining or reattaching to the Thread network.
    Attaching,
    /// Light sleep between checks, attached as a sleepy end device.
    Idle,
    /// Check request in flight.
    Checking,
    /// Panel rail on, frame transfer in progress.
    Fetching { version: u32 },
    /// Waveform running; waiting for BUSY to clear.
    Refreshing { version: u32, waveform: Waveform },
    /// Network unreachable; backing off.
    Offline,
    /// Battery exhausted; deep sleep until USB power returns.
    Exhausted,
}

/// What the app must do next. The core decides; the app only executes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SleepLight {
        seconds: u32,
    },
    SleepDeep {
        seconds: u32,
    },
    SendCheck,
    PanelPowerOn,
    FetchFrame {
        version: u32,
    },
    StartWaveform(Waveform),
    /// Controller sleep command, lines low, load switch off, in that order.
    PanelPowerOff,
    /// Thread rx-on-when-idle for a transfer (`true`) or back to SED polling (`false`).
    SetRxOnWhenIdle(bool),
}
