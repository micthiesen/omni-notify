//! The row codec: how a frame travels as bytes.
//!
//! An encoded frame is a [`FrameHeader`] followed by exactly [`crate::frame::HEIGHT`] row
//! records. Each record starts with a [`RowTag`]. The device decodes one record into a
//! single [`crate::frame::ROW_BYTES`] buffer and writes it to panel RAM, so the decoder needs
//! two rows of RAM (current and previous), never a frame buffer.
//!
//! The device verifies the CRC-32 announced by the check response over the decoded rows
//! before it starts a waveform. A frame that fails any check is discarded with no refresh:
//! writing panel RAM is cheap, the waveform is not.

/// First four bytes of every encoded frame.
pub const MAGIC: [u8; 4] = *b"SLT1";

/// Fixed-size header at the start of an encoded frame. All integers are little-endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// Frame version; must equal the version the check response announced.
    pub version: u32,
    /// Row count; must equal [`crate::frame::HEIGHT`].
    pub rows: u16,
    /// Decoded bytes per row; must equal [`crate::frame::ROW_BYTES`].
    pub row_bytes: u16,
}

/// Encoded length of [`FrameHeader`], including [`MAGIC`].
pub const HEADER_LEN: usize = 4 + 4 + 2 + 2;

/// The first byte of each row record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RowTag {
    /// `ROW_BYTES` literal bytes follow.
    Raw = 0,
    /// PackBits runs follow until exactly `ROW_BYTES` bytes decode. A run that would
    /// overshoot the row is a decode error.
    PackBits = 1,
    /// The row repeats the previous decoded row; nothing follows. Invalid on row 0.
    RepeatPrevious = 2,
}

/// Why a frame was rejected. Any of these ends the fetch without a refresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    BadMagic,
    HeaderMismatch,
    UnknownTag(u8),
    RowOverrun,
    RowUnderrun,
    RepeatOnFirstRow,
    TrailingBytes,
    CrcMismatch,
}
