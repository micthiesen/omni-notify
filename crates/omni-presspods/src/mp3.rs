//! Exact MPEG audio duration, as music-metadata computes it for the episode
//! row (`durationSeconds`).
//!
//! The row stores `frames * samplesPerFrame / sampleRate` with full float
//! precision (for example `168.6465306122449` = 6456 frames at 44.1 kHz):
//! the frame count comes from the Xing/Info (or VBRI) header the final
//! encode writes (`-write_xing 1`), and without one every frame is counted.
//! General tag libraries round to milliseconds or estimate from the bitrate,
//! which would change stored values, chapter end times and the feed's
//! `itunes:duration` rounding, so the header is read directly here.

/// One parsed MPEG audio frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameHeader {
    /// `true` for MPEG-1, `false` for MPEG-2 and 2.5.
    mpeg1: bool,
    layer: u8,
    sample_rate: u32,
    samples_per_frame: u32,
    mono: bool,
    /// Whole frame length in bytes (header included).
    length: usize,
}

const BITRATES_V1: [[u32; 16]; 3] = [
    // Layer I
    [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448, 0,
    ],
    // Layer II
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384, 0,
    ],
    // Layer III
    [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ],
];
const BITRATES_V2: [[u32; 16]; 2] = [
    // Layer I
    [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256, 0,
    ],
    // Layers II and III
    [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ],
];

fn parse_header(bytes: &[u8]) -> Option<FrameHeader> {
    let [b0, b1, b2, b3] = *bytes.get(..4)? else {
        return None;
    };
    if b0 != 0xFF || b1 & 0xE0 != 0xE0 {
        return None;
    }
    let version = (b1 >> 3) & 0b11; // 0 = 2.5, 2 = 2, 3 = 1
    let layer = match (b1 >> 1) & 0b11 {
        0b11 => 1,
        0b10 => 2,
        0b01 => 3,
        _ => return None,
    };
    if version == 1 {
        return None;
    }
    let mpeg1 = version == 3;
    let bitrate_index = usize::from(b2 >> 4);
    let rate_index = usize::from((b2 >> 2) & 0b11);
    let padding = u32::from((b2 >> 1) & 1);
    let base_rate = *[44_100u32, 48_000, 32_000].get(rate_index)?;
    let sample_rate = match version {
        3 => base_rate,
        2 => base_rate / 2,
        _ => base_rate / 4,
    };
    let kbps = if mpeg1 {
        BITRATES_V1[usize::from(layer - 1)][bitrate_index]
    } else {
        BITRATES_V2[usize::from(layer != 1)][bitrate_index]
    };
    if kbps == 0 {
        return None;
    }
    let bitrate = kbps * 1000;
    let (samples_per_frame, length) = match layer {
        1 => (384, (12 * bitrate / sample_rate + padding) * 4),
        2 => (1152, 144 * bitrate / sample_rate + padding),
        _ if mpeg1 => (1152, 144 * bitrate / sample_rate + padding),
        _ => (576, 72 * bitrate / sample_rate + padding),
    };
    Some(FrameHeader {
        mpeg1,
        layer,
        sample_rate,
        samples_per_frame,
        mono: b3 >> 6 == 0b11,
        length: usize::try_from(length).ok()?,
    })
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    let slice = bytes.get(at..at + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// The frame count a Xing/Info or VBRI header declares in the first frame.
fn declared_frames(audio: &[u8], start: usize, header: &FrameHeader) -> Option<u32> {
    if header.layer == 3 {
        let side_info = match (header.mpeg1, header.mono) {
            (true, false) => 32,
            (true, true) | (false, false) => 17,
            (false, true) => 9,
        };
        let at = start + 4 + side_info;
        let tag = audio.get(at..at + 4)?;
        if tag == b"Xing" || tag == b"Info" {
            let flags = read_u32(audio, at + 4)?;
            return if flags & 1 == 1 {
                read_u32(audio, at + 8)
            } else {
                None
            };
        }
    }
    let vbri = start + 4 + 32;
    if audio.get(vbri..vbri + 4) == Some(b"VBRI") {
        return read_u32(audio, vbri + 14);
    }
    None
}

/// Length of a leading ID3v2 tag (header, body and optional footer).
pub(crate) fn id3v2_len(audio: &[u8]) -> usize {
    if audio.len() < 10 || &audio[..3] != b"ID3" {
        return 0;
    }
    let size = audio[6..10]
        .iter()
        .fold(0usize, |acc, b| (acc << 7) | usize::from(b & 0x7f));
    let footer = if audio[5] & 0x10 != 0 { 10 } else { 0 };
    (10 + size + footer).min(audio.len())
}

/// The first frame at or after `from` whose successor (when there is one)
/// also parses, so a stray `0xFF` byte is not mistaken for a sync word.
fn first_frame(audio: &[u8], from: usize) -> Option<(usize, FrameHeader)> {
    (from..audio.len().saturating_sub(3)).find_map(|at| {
        let header = parse_header(&audio[at..])?;
        let next = at + header.length;
        let confirmed = next + 4 > audio.len()
            || parse_header(&audio[next..]).is_some_and(|n| n.sample_rate == header.sample_rate);
        confirmed.then_some((at, header))
    })
}

/// Duration in seconds: declared frames from the Xing/Info or VBRI header,
/// else a count of every frame. `None` when no MPEG audio frame is found.
pub fn duration_seconds(audio: &[u8]) -> Option<f64> {
    let (start, first) = first_frame(audio, id3v2_len(audio))?;
    let frames = match declared_frames(audio, start, &first) {
        Some(frames) => u64::from(frames),
        None => {
            let mut count = 0u64;
            let mut at = start;
            while let Some(header) = audio.get(at..).and_then(parse_header) {
                if header.length == 0 || at + header.length > audio.len() {
                    break;
                }
                count += 1;
                at += header.length;
            }
            count
        }
    };
    #[allow(clippy::cast_precision_loss)]
    let samples = (frames * u64::from(first.samples_per_frame)) as f64;
    Some(samples / f64::from(first.sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 96 kbps MPEG-1 Layer III mono frame header at 44.1 kHz.
    const HEADER: [u8; 4] = [0xFF, 0xFB, 0x70, 0xC4];

    fn frame(len: usize) -> Vec<u8> {
        let mut out = HEADER.to_vec();
        out.resize(len, 0);
        out
    }

    #[test]
    fn parses_a_96k_mono_header() {
        let header = parse_header(&HEADER).unwrap();
        assert_eq!(header.sample_rate, 44_100);
        assert_eq!(header.samples_per_frame, 1152);
        assert!(header.mono && header.mpeg1);
        assert_eq!(header.length, 313);
    }

    #[test]
    fn uses_the_xing_frame_count_with_full_precision() {
        let mut audio = b"ID3\x03\x00\x00\x00\x00\x00\x02xx".to_vec();
        let mut info = frame(313);
        // Mono MPEG-1: side info is 17 bytes after the header.
        info[21..25].copy_from_slice(b"Info");
        info[25..29].copy_from_slice(&1u32.to_be_bytes());
        info[29..33].copy_from_slice(&6456u32.to_be_bytes());
        audio.extend(info);
        audio.extend(frame(313));
        assert_eq!(duration_seconds(&audio), Some(168.6465306122449));
    }

    #[test]
    fn counts_frames_without_a_header() {
        let audio: Vec<u8> = (0..10).flat_map(|_| frame(313)).collect();
        let expected = 10.0 * 1152.0 / 44_100.0;
        assert_eq!(duration_seconds(&audio), Some(expected));
    }

    #[test]
    fn rejects_non_audio() {
        assert_eq!(duration_seconds(b"not an mp3 at all"), None);
    }
}
