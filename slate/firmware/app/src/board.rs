//! Provisional ESP32-H2 pin map. The schematic capture is authoritative once it exists;
//! docs/electrical.md > "Pin map" records the reasoning. Strapping pins (GPIO2, 3, 8, 9,
//! 25), the USB pair (GPIO26, 27) and UART0 TX (GPIO24, which the ROM drives with its boot
//! log) carry no panel signal.

#![allow(dead_code)]

/// TPS22917 ON: panel 3.3 V rail. External pulldown keeps the panel off through reset.
pub const EPD_PWR_EN: u8 = 0;
/// ADC1 channel 0: battery divider midpoint (3.3 MΩ / 1.0 MΩ).
pub const VBAT_SENSE: u8 = 1;
/// Charger status output (open drain, pulled up to 3.3 V).
pub const CHG_STAT: u8 = 4;
/// Second chip select, reserved until the FPC pinout confirms whether the panel has one.
pub const EPD_CS2_N: u8 = 5;
pub const EPD_SCLK: u8 = 10;
pub const EPD_MOSI: u8 = 11;
/// VBUS divider (100 kΩ over 150 kΩ): high while USB power is present. On a pin in the
/// H2's deep-sleep wake range (GPIO7 to 14) so plugging in USB wakes an exhausted or
/// unprovisioned device.
pub const USB_PRESENT: u8 = 12;
pub const EPD_CS_N: u8 = 13;
pub const EPD_DC: u8 = 14;
pub const EPD_RST_N: u8 = 22;
/// Panel BUSY; also a light-sleep wake source while a waveform runs. UART0 RX by default,
/// an input during boot, so the ROM never drives it.
pub const EPD_BUSY: u8 = 23;
