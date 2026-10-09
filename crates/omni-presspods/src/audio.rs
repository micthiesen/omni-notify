//! Episode MP3 metadata (`src/press-pods/audio.ts`): duration and ID3 tags.
//!
//! Tagging embeds the article's lead image as album art (fetched
//! best-effort) and ID3 chapters (CHAP frames plus a top-level, ordered CTOC)
//! so podcast apps show a scrubbable chapter list. It is best-effort as a
//! whole: any failure returns the audio untouched rather than losing the
//! episode. Tags are written as ID3v2.3, replacing any existing tag, like
//! node-id3.

use std::time::Duration;

use id3::frame::{Chapter as ChapterFrame, Picture, PictureType, TableOfContents};
use id3::{Frame, Tag, TagLike as _, Version};
use omni_http::public::PublicHttpClient;

use crate::error::PressPodsError;
use crate::model::Chapter;
use crate::public_http::{PRESS_PODS_IMAGE_MAX_BYTES, PublicGet, fetch_public_buffer};

const LOG: &str = "PressPods";

/// `getDuration`: the MP3's duration in seconds (exact frame arithmetic, see
/// [`crate::mp3`]), or `None` (logged at ERROR) when it cannot be read.
pub fn audio_duration_seconds(audio: &[u8]) -> Option<f64> {
    let duration = crate::mp3::duration_seconds(audio);
    if duration.is_none() {
        tracing::error!(target: LOG, error = "no MPEG audio frames found", "Error getting audio duration:");
    }
    duration
}

/// Album art fetched for tagging.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumArt {
    pub mime: String,
    pub data: Vec<u8>,
}

/// Fetches the lead image (public client, 20 s, no retry, 10 MiB). `None`
/// (warned) when the fetch fails or the response is not an image.
pub async fn fetch_album_art(client: &PublicHttpClient, url: &str) -> Option<AlbumArt> {
    let result = fetch_public_buffer(
        client,
        &PublicGet {
            url,
            headers: vec![("accept", "image/*".to_owned())],
            timeout: Duration::from_secs(20),
            retries: 0,
            max_bytes: PRESS_PODS_IMAGE_MAX_BYTES,
            operation: "fetch PressPods album art",
        },
    )
    .await;
    match result {
        Ok(response) => {
            let mime = response
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            match mime.filter(|m| m.contains("image")) {
                Some(mime) => Some(AlbumArt {
                    // Trust the header; the URL path often carries query strings.
                    mime: mime.split(';').next().unwrap_or("").trim().to_owned(),
                    data: response.body.to_vec(),
                }),
                None => {
                    tracing::warn!(target: LOG, error = "No image mime type", "Error fetching album art:");
                    None
                }
            }
        }
        Err(error) => {
            tracing::warn!(target: LOG, %error, "Error fetching album art:");
            None
        }
    }
}

/// A CHAP frame per chapter plus the CTOC referencing them; needs at least
/// two chapters. A chapter ends where the next starts, the last at the
/// episode duration (or one second after it starts when unknown).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn chapter_frames(
    chapters: &[Chapter],
    duration_seconds: Option<f64>,
) -> Option<(Vec<ChapterFrame>, TableOfContents)> {
    if chapters.len() < 2 {
        return None;
    }
    let ms = |seconds: f64| (seconds * 1000.0).round().max(0.0) as u32;
    let total_ms = duration_seconds.filter(|d| *d != 0.0).map(ms);
    let frames: Vec<ChapterFrame> = chapters
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let start_time = ms(chapter.start_time_seconds);
            let end_time = match chapters.get(i + 1) {
                Some(next) => ms(next.start_time_seconds),
                None => total_ms.unwrap_or(start_time + 1000),
            };
            ChapterFrame {
                element_id: format!("chp{i}"),
                start_time,
                end_time,
                start_offset: 0xFFFF_FFFF,
                end_offset: 0xFFFF_FFFF,
                frames: vec![Frame::text("TIT2", chapter.title.clone())],
            }
        })
        .collect();
    let toc = TableOfContents {
        element_id: "toc".to_owned(),
        top_level: true,
        ordered: true,
        elements: frames.iter().map(|f| f.element_id.clone()).collect(),
        frames: Vec::new(),
    };
    Some((frames, toc))
}

/// Writes the tags; `Err` leaves the caller to fall back to untagged audio.
pub fn write_tags(
    audio: &[u8],
    art: Option<&AlbumArt>,
    chapters: &[Chapter],
    duration_seconds: Option<f64>,
) -> Result<Option<Vec<u8>>, PressPodsError> {
    let mut tag = Tag::new();
    let mut any = false;
    if let Some(art) = art {
        tag.add_frame(Picture {
            mime_type: art.mime.clone(),
            picture_type: PictureType::CoverFront,
            description: "Cover".to_owned(),
            data: art.data.clone(),
        });
        any = true;
    }
    if let Some((frames, toc)) = chapter_frames(chapters, duration_seconds) {
        for frame in frames {
            tag.add_frame(frame);
        }
        tag.add_frame(toc);
        any = true;
    }
    if !any {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(audio.len() + 64 * 1024);
    tag.write_to(&mut out, Version::Id3v23)
        .map_err(|e| PressPodsError::failed("write PressPods ID3 tags", e.to_string()))?;
    out.extend_from_slice(&audio[crate::mp3::id3v2_len(audio)..]);
    Ok(Some(out))
}

/// `tagEpisodeAudio`: album art and chapters; never fails.
pub async fn tag_episode_audio(
    client: &PublicHttpClient,
    audio: Vec<u8>,
    lead_image_url: Option<&str>,
    chapters: &[Chapter],
    duration_seconds: Option<f64>,
) -> Vec<u8> {
    let art = match lead_image_url.filter(|u| !u.is_empty()) {
        Some(url) => fetch_album_art(client, url).await,
        None => None,
    };
    let has_art = art.is_some();
    match write_tags(&audio, art.as_ref(), chapters, duration_seconds) {
        Ok(Some(tagged)) => {
            let count = chapter_frames(chapters, duration_seconds).map_or(0, |(f, _)| f.len());
            tracing::info!(target: LOG, art = has_art, chapters = count, "Embedded ID3 tags");
            tagged
        }
        Ok(None) => audio,
        Err(error) => {
            tracing::warn!(target: LOG, %error, "Error writing ID3 tags:");
            audio
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_frames_follow_node_id3() {
        let chapters = vec![Chapter::new(1.5, "Intro"), Chapter::new(64.2504, "Part")];
        let (frames, toc) = chapter_frames(&chapters, Some(100.0)).unwrap();
        assert_eq!(frames[0].start_time, 1500);
        assert_eq!(frames[0].end_time, 64250);
        assert_eq!(frames[1].end_time, 100_000);
        assert_eq!(frames[1].element_id, "chp1");
        assert_eq!(toc.elements, ["chp0", "chp1"]);
        assert!(toc.top_level && toc.ordered);
        assert!(chapter_frames(&chapters[..1], Some(10.0)).is_none());
        let (frames, _) = chapter_frames(&chapters, None).unwrap();
        assert_eq!(frames[1].end_time, 65250);
    }

    #[test]
    fn tags_replace_an_existing_id3_header() {
        // A minimal ID3v2.4 header with a 4-byte body, then "audio".
        let mut audio = b"ID3\x04\x00\x00\x00\x00\x00\x04abcd".to_vec();
        audio.extend_from_slice(b"audio");
        let chapters = vec![Chapter::new(0.0, "A"), Chapter::new(1.0, "B")];
        let art = AlbumArt {
            mime: "image/jpeg".into(),
            data: vec![1, 2, 3],
        };
        let tagged = write_tags(&audio, Some(&art), &chapters, Some(2.0))
            .unwrap()
            .unwrap();
        assert!(tagged.ends_with(b"audio"));
        assert!(!tagged.windows(4).any(|w| w == b"abcd"));
        let tag = Tag::read_from2(std::io::Cursor::new(&tagged)).unwrap();
        assert_eq!(tag.version(), Version::Id3v23);
        assert_eq!(tag.pictures().count(), 1);
        assert_eq!(tag.chapters().count(), 2);
        assert_eq!(tag.tables_of_contents().count(), 1);
        assert!(write_tags(&audio, None, &[], None).unwrap().is_none());
    }
}
