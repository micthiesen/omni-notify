//! Panel geometry and ink codes for the Good Display GDEY133F91 (SSD2677 controller).

/// Panel width in pixels, in the controller's RAM row direction.
///
/// Provisional: the SSD2677 RAM scan direction for this panel is unconfirmed. If the
/// controller scans the 680-pixel edge, swap these and the row size follows (gate G-01).
pub const WIDTH: u16 = 960;
/// Panel height in pixels (rows in controller RAM order).
pub const HEIGHT: u16 = 680;
/// Bits per pixel: four inks need two bits.
pub const BITS_PER_PIXEL: u16 = 2;
/// Bytes in one decoded row.
pub const ROW_BYTES: u16 = WIDTH * BITS_PER_PIXEL / 8;
/// Bytes in one decoded frame.
pub const FRAME_BYTES: u32 = ROW_BYTES as u32 * HEIGHT as u32;

const _: () = assert!(ROW_BYTES == 240);
const _: () = assert!(FRAME_BYTES == 163_200);

/// The four inks of the E Ink Spectra 3100 panel.
///
/// The discriminants are the controller-native two-bit codes. They are provisional until
/// checked against the GDEY133F91 specification and a test pattern on a real panel; the
/// server encoder and the device must use the same table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Ink {
    Black = 0b00,
    White = 0b01,
    Yellow = 0b10,
    Red = 0b11,
}

/// Which panel waveform the server asks the device to run for a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Waveform {
    /// Specified 25 s global update. The default until fast refresh passes its quality gate.
    Full = 0,
    /// Specified 20 s global update. Allowed only after `PANEL-FAST-QUALITY` passes.
    Fast = 1,
}
