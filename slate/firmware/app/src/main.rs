//! Omni Slate firmware for the ESP32-H2-MINI-1-H4S.
//!
//! Scaffold: boots, logs, and idles. The behaviour lives in `slate-core`; this binary will
//! only wire it to the SPI panel, the load switch, the ADC and Thread. Pin assignments are
//! in `board.rs` and docs/electrical.md.

#![no_std]
#![no_main]

mod board;

use esp_backtrace as _;
use esp_hal::delay::Delay;
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_hal::main]
fn main() -> ! {
    esp_println::logger::init_logger_from_env();
    let _peripherals = esp_hal::init(esp_hal::Config::default());

    info!(
        "Omni Slate scaffold: panel {}x{}, firmware not implemented",
        slate_protocol::frame::WIDTH,
        slate_protocol::frame::HEIGHT
    );

    let delay = Delay::new();
    loop {
        delay.delay_millis(60_000);
    }
}
