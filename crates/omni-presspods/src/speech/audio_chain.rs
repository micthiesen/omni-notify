//! Audio assembly; read
//! `docs/presspods-audio.md` before changing anything here.
//!
//! Per chunk: speed up, optionally denoise (Higgs), trim edge silence, fade
//! the edges, and level to -19 LUFS with two-pass linear loudnorm. Per
//! episode: concat, master to -16 LUFS / -1.5 dBTP, then join the intro and
//! encode once to 96k mono MP3.
//!
//! Invariants:
//! - every sample-rate conversion carries [`RESAMPLE_HQ`], including explicit
//!   conversions that pre-empt ffmpeg's auto-inserted default-quality ones
//!   (before `arnndn`, after each loudnorm, both intro branches);
//! - the Higgs denoise path is `highpass`, HQ resample to 48k, `arnndn` with
//!   `assets/press-pods/denoise.rnnn`, then [`FIZZ_SHELF`].
//!
//! The argument builders are pure so tests can check every filter string;
//! the runner owns its temporary files ([`TempFile`] removes on drop), so an
//! interrupted assembly kills ffmpeg (`kill_on_drop`) and leaves nothing behind.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use omni_core::js::number_to_string;
use regex::Regex;
use serde::Deserialize;

use crate::error::PressPodsError;
use crate::storage::random_hex;

pub const SAMPLE_RATE: u32 = 44_100;
/// Narration playback speed (pitch-preserving `atempo`), applied first so every
/// duration and chapter offset reflects the sped audio. The intro is not sped.
pub const SPEED_MULTIPLIER: f64 = 1.1;
/// Per-chunk leveling target; the master lifts everything to -16 LUFS.
pub const CHUNK_LUFS: i32 = -19;
/// Delivery target: -16 LUFS / -1.5 dBTP (podcast convention).
pub const MASTER_LUFS: i32 = -16;
/// Short fades at each chunk edge so butt-joins never click.
pub const EDGE_FADE_SEC: f64 = 0.012;
/// swresample's default anti-imaging filter mirrors Higgs's ~10.7 kHz band edge
/// into an audible ~13 kHz ring; these parameters bury the images below the
/// noise floor. Every `aresample` here must carry them.
pub const RESAMPLE_HQ: &str = "filter_size=256:cutoff=0.95";
/// The RNNoise model, relative to the assets directory.
pub const DENOISE_MODEL_ASSET: &str = "press-pods/denoise.rnnn";
/// Steep linear-phase FIR shelf removing Higgs's exposed 9.8-11 kHz sibilance
/// comb while leaving content at or below 9.5 kHz untouched.
pub const FIZZ_SHELF: &str =
    "firequalizer=gain='if(lt(f,9600),0,if(gt(f,10300),-30,-30*(f-9600)/700))'";

/// Upper bound on one ffmpeg/ffprobe invocation.
const PROCESS_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const STDOUT_CAP: usize = 1024 * 1024;
/// Captured stderr cap per process.
const STDERR_CAP: usize = 128 * 1024 * 1024;

static LOUDNORM_JSON: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r#"\{[^{}]*"input_i"(?s:.)*?\}"#).ok());

/// `DENOISE_FILTER`: rumble cut, explicit HQ upsample to RNNoise's 48 kHz,
/// RNNoise, then the fizz shelf.
pub fn denoise_filter(model_path: &str) -> String {
    format!("highpass=f=80,aresample=48000:{RESAMPLE_HQ},arnndn=m={model_path},{FIZZ_SHELF}")
}

/// The per-chunk edge chain: speed, optional denoise, `areverse`-sandwiched
/// silence trim and fades, and an HQ resample to the output rate.
pub fn chunk_edge_filter(denoise_model: Option<&str>) -> String {
    let fade = number_to_string(EDGE_FADE_SEC);
    let mut filter = format!("atempo={},", number_to_string(SPEED_MULTIPLIER));
    if let Some(model) = denoise_model {
        filter.push_str(&denoise_filter(model));
        filter.push(',');
    }
    filter.push_str(&format!(
        "silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.15,\
         afade=t=in:st=0:d={fade},\
         areverse,\
         silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.25,\
         afade=t=in:st=0:d={fade},\
         areverse,\
         aresample={SAMPLE_RATE}:{RESAMPLE_HQ}"
    ));
    filter
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// ffmpeg arguments (after `-hide_banner -y`) for the chunk edge pass.
pub fn prepare_chunk_args(raw: &Path, trimmed: &Path, denoise_model: Option<&str>) -> Vec<String> {
    let mut out = args(&["-i"]);
    out.push(path_arg(raw));
    out.extend(args(&["-af"]));
    out.push(chunk_edge_filter(denoise_model));
    out.extend(args(&["-ar", "44100", "-ac", "1"]));
    out.push(path_arg(trimmed));
    out
}

fn loudnorm_spec(target: i32) -> String {
    format!("I={target}:TP=-1.5:LRA=11")
}

/// Loudnorm pass 1 (measure only).
pub fn loudnorm_measure_args(input: &Path, target: i32) -> Vec<String> {
    let mut out = args(&["-i"]);
    out.push(path_arg(input));
    out.push("-af".to_owned());
    out.push(format!(
        "loudnorm={}:print_format=json",
        loudnorm_spec(target)
    ));
    out.extend(args(&["-f", "null", "-"]));
    out
}

/// Loudnorm pass 1 output.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LoudnormMeasurement {
    pub input_i: String,
    pub input_tp: String,
    pub input_lra: String,
    pub input_thresh: String,
    pub target_offset: String,
}

/// Parses the JSON block loudnorm prints to stderr.
pub fn parse_loudnorm(stderr: &str, input: &Path) -> Result<LoudnormMeasurement, PressPodsError> {
    let block = LOUDNORM_JSON
        .as_ref()
        .and_then(|re| re.find(stderr))
        .ok_or_else(|| {
            PressPodsError::invalid(
                "decode ffmpeg loudnorm measurement",
                format!("loudnorm measurement failed for {}", input.display()),
            )
        })?;
    let value: serde_json::Value = serde_json::from_str(block.as_str())
        .map_err(|e| PressPodsError::invalid("parse loudnorm JSON", e.to_string()))?;
    serde_json::from_value(value)
        .map_err(|e| PressPodsError::invalid("decode loudnorm JSON", e.to_string()))
}

/// Loudnorm pass 2: one static linear gain, then an HQ resample (loudnorm
/// always runs a 192 kHz round trip internally).
pub fn loudnorm_apply_filter(target: i32, m: &LoudnormMeasurement) -> String {
    format!(
        "loudnorm={}:linear=true:measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={},aresample={SAMPLE_RATE}:{RESAMPLE_HQ}",
        loudnorm_spec(target),
        m.input_i,
        m.input_tp,
        m.input_lra,
        m.input_thresh,
        m.target_offset
    )
}

/// Loudnorm pass 2 arguments (WAV or 96k MP3 output).
pub fn loudnorm_apply_args(
    input: &Path,
    output: &Path,
    target: i32,
    m: &LoudnormMeasurement,
    to_wav: bool,
) -> Vec<String> {
    let mut out = args(&["-i"]);
    out.push(path_arg(input));
    out.push("-af".to_owned());
    out.push(loudnorm_apply_filter(target, m));
    out.extend(args(&["-ar", "44100", "-ac", "1"]));
    if to_wav {
        out.extend(args(&["-c:a", "pcm_s16le"]));
    } else {
        out.extend(args(&["-c:a", "libmp3lame", "-b:a", "96k"]));
    }
    out.push(path_arg(output));
    out
}

/// JS `Number#toFixed(3)` for a non-negative gap length.
fn to_fixed3(seconds: f64) -> String {
    format!("{seconds:.3}")
}

/// A silence WAV of the given length.
pub fn silence_args(seconds: f64, output: &Path) -> Vec<String> {
    let mut out = args(&["-f", "lavfi", "-i", "anullsrc=r=44100:cl=mono", "-t"]);
    out.push(to_fixed3(seconds));
    out.push(path_arg(output));
    out
}

/// The concat demuxer manifest (`file '<path>'`, quotes escaped).
pub fn concat_manifest(files: &[&Path]) -> String {
    files
        .iter()
        .map(|f| format!("file '{}'", path_arg(f).replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Stream-copy concat of same-format WAVs.
pub fn concat_args(manifest: &Path, output: &Path) -> Vec<String> {
    let mut out = args(&["-f", "concat", "-safe", "0", "-i"]);
    out.push(path_arg(manifest));
    out.extend(args(&["-c", "copy"]));
    out.push(path_arg(output));
    out
}

/// The intro join: both inputs conformed to 44.1k mono with HQ resampling,
/// the intro loudness-matched, then concatenated.
pub fn assemble_filter_complex() -> String {
    format!(
        "[0:a]aresample={SAMPLE_RATE}:{RESAMPLE_HQ},aformat=channel_layouts=mono,\
         loudnorm=I={MASTER_LUFS}:TP=-1.5:LRA=11,\
         aresample={SAMPLE_RATE}:{RESAMPLE_HQ}[intro];\
         [1:a]aresample={SAMPLE_RATE}:{RESAMPLE_HQ},aformat=channel_layouts=mono[speech];\
         [intro][speech]concat=n=2:v=0:a=1[out]"
    )
}

/// The single final encode: intro + mastered speech to 96k mono MP3 with a Xing header.
pub fn assemble_args(intro: &Path, speech: &Path, output: &Path) -> Vec<String> {
    let mut out = args(&["-i"]);
    out.push(path_arg(intro));
    out.push("-i".to_owned());
    out.push(path_arg(speech));
    out.push("-filter_complex".to_owned());
    out.push(assemble_filter_complex());
    out.extend(args(&[
        "-map",
        "[out]",
        "-ar",
        "44100",
        "-ac",
        "1",
        "-c:a",
        "libmp3lame",
        "-b:a",
        "96k",
        "-write_xing",
        "1",
    ]));
    out.push(path_arg(output));
    out
}

/// ffprobe arguments printing the container duration.
pub fn probe_args(file: &Path) -> Vec<String> {
    let mut out = args(&[
        "-v",
        "error",
        "-show_entries",
        "format=duration",
        "-of",
        "csv=p=0",
    ]);
    out.push(path_arg(file));
    out
}

/// A temporary file (`pp_<hex>.<ext>`) removed when dropped, so every exit
/// path (error, interruption, success) cleans up.
#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    /// Reserves a fresh name in `dir` (the file is created by whoever writes it).
    pub fn reserve(dir: &Path, ext: &str) -> Self {
        Self {
            path: dir.join(format!("pp_{}.{ext}", random_hex(8))),
        }
    }

    /// A fresh file holding `bytes`.
    pub async fn write(dir: &Path, ext: &str, bytes: &[u8]) -> Result<Self, PressPodsError> {
        let file = Self::reserve(dir, ext);
        tokio::fs::write(&file.path, bytes)
            .await
            .map_err(|e| PressPodsError::io("write temporary audio file", e))?;
        Ok(file)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        // Best-effort; the name may never have been created.
        let _ignored = std::fs::remove_file(&self.path);
    }
}

/// A concat-ready chunk WAV and its duration (chapter offsets derive from it).
#[derive(Debug)]
pub struct PreparedChunk {
    pub wav: TempFile,
    pub duration_seconds: f64,
}

/// Runs ffmpeg and ffprobe.
#[derive(Clone, Debug)]
pub struct AudioChain {
    ffmpeg: String,
    ffprobe: String,
    /// Absolute path of the RNNoise model, as written into the filtergraph.
    denoise_model: String,
    tmp_dir: PathBuf,
}

/// Characters with meaning in an ffmpeg filtergraph or option list.
const FILTERGRAPH_SPECIAL: &[char] = &[':', ',', ';', '[', ']', '\'', '\\', '=', ' '];

impl AudioChain {
    /// Fails when the RNNoise model path would need filtergraph escaping.
    pub fn new(
        ffmpeg: impl Into<String>,
        ffprobe: impl Into<String>,
        denoise_model: &Path,
        tmp_dir: PathBuf,
    ) -> Result<Self, PressPodsError> {
        let model = path_arg(denoise_model);
        if model.contains(FILTERGRAPH_SPECIAL) {
            return Err(PressPodsError::invalid(
                "configure PressPods audio chain",
                format!("denoise model path must not contain filtergraph metacharacters: {model}"),
            ));
        }
        Ok(Self {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
            denoise_model: model,
            tmp_dir,
        })
    }

    pub fn tmp_dir(&self) -> &Path {
        &self.tmp_dir
    }

    pub fn denoise_model(&self) -> &str {
        &self.denoise_model
    }

    async fn run(
        &self,
        program: &str,
        operation: &str,
        arguments: Vec<String>,
    ) -> Result<omni_core::process::Output, PressPodsError> {
        let mut command = tokio::process::Command::new(program);
        command.args(&arguments);
        let output =
            omni_core::process::run_bounded(command, None, STDOUT_CAP, STDERR_CAP, PROCESS_TIMEOUT)
                .await
                .map_err(|e| PressPodsError::Process {
                    operation: operation.to_owned(),
                    message: e.to_string(),
                    source: Some(e),
                })?;
        if output.status != 0 {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr
                .chars()
                .rev()
                .take(500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            return Err(PressPodsError::Process {
                operation: operation.to_owned(),
                message: format!(
                    "{program} exited with status {}: {}",
                    output.status,
                    tail.trim()
                ),
                source: None,
            });
        }
        Ok(output)
    }

    /// `ffmpeg -hide_banner -y <args>`; returns stderr.
    async fn ffmpeg(&self, arguments: Vec<String>) -> Result<String, PressPodsError> {
        let mut full = args(&["-hide_banner", "-y"]);
        full.extend(arguments);
        let output = self.run(&self.ffmpeg, "run ffmpeg", full).await?;
        Ok(String::from_utf8_lossy(&output.stderr).into_owned())
    }

    /// The container duration in seconds (finite, non-negative).
    pub async fn probe_duration_seconds(&self, file: &Path) -> Result<f64, PressPodsError> {
        let output = self
            .run(&self.ffprobe, "probe audio duration", probe_args(file))
            .await?;
        let text = String::from_utf8_lossy(&output.stdout);
        let duration = omni_core::js::string_to_number(text.trim());
        if duration.is_finite() && duration >= 0.0 {
            Ok(duration)
        } else {
            Err(PressPodsError::invalid(
                "decode ffprobe duration",
                format!(
                    "Expected a finite non-negative duration, got {:?}",
                    text.trim()
                ),
            ))
        }
    }

    async fn two_pass_loudnorm(
        &self,
        input: &Path,
        output: &Path,
        target: i32,
        to_wav: bool,
    ) -> Result<(), PressPodsError> {
        let stderr = self.ffmpeg(loudnorm_measure_args(input, target)).await?;
        let measurement = parse_loudnorm(&stderr, input)?;
        self.ffmpeg(loudnorm_apply_args(
            input,
            output,
            target,
            &measurement,
            to_wav,
        ))
        .await
        .map(|_| ())
    }

    /// One raw TTS chunk (MP3 bytes) to a concat-ready, leveled WAV.
    pub async fn prepare_chunk(
        &self,
        mp3: &[u8],
        denoise: bool,
    ) -> Result<PreparedChunk, PressPodsError> {
        let raw = TempFile::write(&self.tmp_dir, "mp3", mp3).await?;
        let trimmed = TempFile::reserve(&self.tmp_dir, "wav");
        let wav = TempFile::reserve(&self.tmp_dir, "wav");
        let model = denoise.then_some(self.denoise_model.as_str());
        self.ffmpeg(prepare_chunk_args(raw.path(), trimmed.path(), model))
            .await?;
        self.two_pass_loudnorm(trimmed.path(), wav.path(), CHUNK_LUFS, true)
            .await?;
        let duration_seconds = self.probe_duration_seconds(wav.path()).await?;
        Ok(PreparedChunk {
            wav,
            duration_seconds,
        })
    }

    /// Materializes cached checkpoint bytes as a temporary WAV and probes it;
    /// a corrupt checkpoint fails here (and its file is removed).
    pub async fn resume_chunk(&self, wav: &[u8]) -> Result<PreparedChunk, PressPodsError> {
        let file = TempFile::write(&self.tmp_dir, "wav", wav).await?;
        let duration_seconds = self.probe_duration_seconds(file.path()).await?;
        Ok(PreparedChunk {
            wav: file,
            duration_seconds,
        })
    }

    /// A silence WAV used between chunks and sections.
    pub async fn make_silence_wav(&self, seconds: f64) -> Result<TempFile, PressPodsError> {
        let out = TempFile::reserve(&self.tmp_dir, "wav");
        self.ffmpeg(silence_args(seconds, out.path())).await?;
        Ok(out)
    }

    async fn concat_wavs(&self, files: &[&Path]) -> Result<TempFile, PressPodsError> {
        let manifest =
            TempFile::write(&self.tmp_dir, "txt", concat_manifest(files).as_bytes()).await?;
        let out = TempFile::reserve(&self.tmp_dir, "wav");
        self.ffmpeg(concat_args(manifest.path(), out.path()))
            .await?;
        Ok(out)
    }

    /// Concat, master to -16 LUFS, join the intro and encode once.
    pub async fn assemble_episode(
        &self,
        chunk_wavs: &[&Path],
        intro_mp3: &[u8],
    ) -> Result<Vec<u8>, PressPodsError> {
        let speech_raw = self.concat_wavs(chunk_wavs).await?;
        let mastered = TempFile::reserve(&self.tmp_dir, "wav");
        self.two_pass_loudnorm(speech_raw.path(), mastered.path(), MASTER_LUFS, true)
            .await?;
        let intro = TempFile::write(&self.tmp_dir, "mp3", intro_mp3).await?;
        let out = TempFile::reserve(&self.tmp_dir, "mp3");
        self.ffmpeg(assemble_args(intro.path(), mastered.path(), out.path()))
            .await?;
        tokio::fs::read(out.path())
            .await
            .map_err(|e| PressPodsError::io("read assembled episode", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `aresample=` must be followed by the rate and [`RESAMPLE_HQ`].
    pub(crate) fn assert_resample_hq(filter: &str) {
        for (i, _) in filter.match_indices("aresample=") {
            let rest = &filter[i + "aresample=".len()..];
            let rate_end = rest.find(':').unwrap_or(rest.len());
            assert!(
                rest[rate_end..].starts_with(&format!(":{RESAMPLE_HQ}")),
                "aresample without RESAMPLE_HQ in {filter}"
            );
        }
    }

    #[test]
    fn higgs_chunk_filter_is_pinned() {
        assert_eq!(
            chunk_edge_filter(Some("assets/press-pods/denoise.rnnn")),
            "atempo=1.1,highpass=f=80,aresample=48000:filter_size=256:cutoff=0.95,\
             arnndn=m=assets/press-pods/denoise.rnnn,\
             firequalizer=gain='if(lt(f,9600),0,if(gt(f,10300),-30,-30*(f-9600)/700))',\
             silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.15,\
             afade=t=in:st=0:d=0.012,areverse,\
             silenceremove=start_periods=1:start_threshold=-45dB:start_silence=0.25,\
             afade=t=in:st=0:d=0.012,areverse,aresample=44100:filter_size=256:cutoff=0.95"
        );
    }

    #[test]
    fn clean_provider_chunk_filter_has_no_denoise() {
        let filter = chunk_edge_filter(None);
        assert!(!filter.contains("arnndn"));
        assert!(!filter.contains("firequalizer"));
        assert!(filter.starts_with("atempo=1.1,silenceremove="));
        assert_resample_hq(&filter);
    }

    #[test]
    fn every_filter_resamples_with_hq_parameters() {
        let m = LoudnormMeasurement {
            input_i: "-20.1".into(),
            input_tp: "-3.2".into(),
            input_lra: "4.5".into(),
            input_thresh: "-30.4".into(),
            target_offset: "0.3".into(),
        };
        for filter in [
            chunk_edge_filter(Some("m.rnnn")),
            loudnorm_apply_filter(CHUNK_LUFS, &m),
            loudnorm_apply_filter(MASTER_LUFS, &m),
            assemble_filter_complex(),
            denoise_filter("m.rnnn"),
        ] {
            assert_resample_hq(&filter);
        }
        assert_eq!(
            loudnorm_apply_filter(CHUNK_LUFS, &m),
            "loudnorm=I=-19:TP=-1.5:LRA=11:linear=true:measured_I=-20.1:measured_TP=-3.2:measured_LRA=4.5:measured_thresh=-30.4:offset=0.3,aresample=44100:filter_size=256:cutoff=0.95"
        );
        assert_eq!(
            assemble_filter_complex(),
            "[0:a]aresample=44100:filter_size=256:cutoff=0.95,aformat=channel_layouts=mono,loudnorm=I=-16:TP=-1.5:LRA=11,aresample=44100:filter_size=256:cutoff=0.95[intro];[1:a]aresample=44100:filter_size=256:cutoff=0.95,aformat=channel_layouts=mono[speech];[intro][speech]concat=n=2:v=0:a=1[out]"
        );
    }

    #[test]
    fn parses_the_loudnorm_measurement_block() {
        let stderr = "[Parsed_loudnorm_0 @ 0x1]\n{\n\t\"input_i\" : \"-27.61\",\n\t\"input_tp\" : \"-4.47\",\n\t\"input_lra\" : \"18.06\",\n\t\"input_thresh\" : \"-39.20\",\n\t\"output_i\" : \"-16.58\",\n\t\"target_offset\" : \"0.58\"\n}\n";
        let m = parse_loudnorm(stderr, Path::new("x.wav")).unwrap();
        assert_eq!(m.input_i, "-27.61");
        assert_eq!(m.target_offset, "0.58");
        assert!(parse_loudnorm("no json here", Path::new("x.wav")).is_err());
    }

    #[test]
    fn manifest_and_silence_arguments_are_pinned() {
        assert_eq!(
            concat_manifest(&[Path::new("/tmp/a.wav"), Path::new("/tmp/it's.wav")]),
            "file '/tmp/a.wav'\nfile '/tmp/it'\\''s.wav'"
        );
        assert_eq!(
            silence_args(0.7, Path::new("o.wav")),
            [
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=mono",
                "-t",
                "0.700",
                "o.wav"
            ]
        );
    }

    #[test]
    fn rejects_a_model_path_that_needs_escaping() {
        assert!(
            AudioChain::new(
                "ffmpeg",
                "ffprobe",
                Path::new("/a:b/m.rnnn"),
                PathBuf::from("/tmp")
            )
            .is_err()
        );
        assert!(
            AudioChain::new(
                "ffmpeg",
                "ffprobe",
                Path::new("/app/assets/press-pods/denoise.rnnn"),
                PathBuf::from("/tmp")
            )
            .is_ok()
        );
    }
}
