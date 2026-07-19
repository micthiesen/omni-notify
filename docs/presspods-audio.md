# PressPods TTS audio: models and post-processing

The post-processing chain (`src/press-pods/speech/audioChain.ts`) is where most
of the perceived quality of an episode is won or lost, and it's an ongoing
refinement target. This doc records what the chain does, why each stage exists,
the artifacts we've found and fixed (with the measurement method, so future
issues can be diagnosed the same way), and the known next steps.

## TTS providers and what their raw audio looks like

| Provider | Native output | Bandwidth | Noise floor | Quirks |
|---|---|---|---|---|
| Higgs v3 (self-hosted mlx-audio, default) | 24 kHz MP3 ~128k mono | content shelf ~11 kHz | audible hiss (needs denoise) | random voice per request unless ref-cloned; unreliable length (truncation/runaway); dense vocoder "fizz" peaks at 10.0–10.7 kHz right below its band edge |
| ElevenLabs v3 | 44.1 kHz MP3 | full | clean | none of the above; costs $0.10/1k chars |

Two provider flags drive the chain: `needsDenoise` (Higgs) and
`verifyChunkLength` (Higgs; length-retry lives in `synthesize.ts`, not the
audio chain).

The key fact about Higgs: **everything above ~11 kHz in a finished episode is
by definition an artifact**, because the model itself produces nothing up
there. That makes the high band a clean diagnostic window — any energy that
shows up above the shelf was manufactured by our own processing.

## The chain

Per chunk (`prepareChunk`):

1. **Denoise** (Higgs only): `highpass=f=80` rumble cut, then RNNoise
   (`arnndn` with `assets/press-pods/denoise.rnnn`). RNNoise over `afftdn`
   because spectral subtraction leaves a metallic "musical noise" tang on
   voice. RNNoise only runs at 48 kHz, so the filter chain upsamples to 48k
   first (see the resampling section below).
2. **Edge trim + fades**: `silenceremove` at each end (via an `areverse`
   sandwich), 12 ms fades so butt-joins never click.
3. **Per-chunk leveling** to −19 LUFS (two-pass *linear* loudnorm) so no chunk
   sits quieter than its neighbors.

Per episode (`assembleEpisode`):

4. **Concat** chunk/gap WAVs (stream copy, no re-encode).
5. **Master** to −16 LUFS / −1.5 dBTP (two-pass linear loudnorm — the podcast
   delivery convention).
6. **Intro join + single encode**: intro jingle loudness-matched and
   concatenated in one filtergraph, one 96k mono MP3 encode. A single final
   encode replaces the old three-generation MP3 chain.

Two-pass *linear* loudnorm everywhere: pass 1 measures, pass 2 applies one
static gain. One-pass loudnorm runs a dynamic AGC that pumps and lifts quiet
passages. Note that when the linear gain would violate TP=−1.5, loudnorm
scales back / falls back internally — episodes land around −17 LUFS on peaky
speech instead of clipping. Don't "fix" that by replacing the apply-pass with
a plain `volume` gain: we tested it, and the TP clamp makes it undershoot
loudness by ~3 dB with no audible benefit.

## Fixed: the ~13 kHz sibilance "ring" (resampler imaging, 2026-07)

**Symptom**: a faint, high-pitched metallic ring shadowing every sibilant —
"a spoon rattling very quietly in a ceramic cup." Peaked at −37 dBFS in a
−16 LUFS episode; clearly audible on headphones.

**Diagnosis** (the method matters more than the instance):

- Spectrogram of the episode showed speech content ending hard at ~11 kHz
  (Higgs's shelf) with a persistent speckled band at **~13.26 kHz** riding
  above it — 20 dB of spectral prominence.
- STFT band-energy correlation: the 13.26 kHz band tracked the sibilance band
  (8.3–9.5 kHz) at r=0.70, but ~0 against overall speech energy or a control
  band. So: a sibilance-shadow, not noise.
- A raw chunk fetched straight from the mlx server was **clean** above
  11.5 kHz (−140 dB). The artifact was created by us.
- Stage-by-stage replay found it in the very first step: a bare
  `ffmpeg -i raw.mp3 -ar 44100` already produced the bump. **swresample's
  default anti-imaging filter is weak**; upsampling 24 kHz output mirrors the
  vocoder's 10.7 kHz band-edge energy around 12 kHz → ~13.3 kHz images at
  only ~35 dB below source. The chain then compounded this across its many
  conversions: 24k→48k (auto-inserted for `arnndn`), 48k→44.1k out, and a
  192 kHz round-trip *inside every loudnorm pass* (loudnorm always resamples
  internally, even in linear mode).

**Fix**: every `aresample` in `audioChain.ts` carries
`filter_size=256:cutoff=0.95` (`RESAMPLE_HQ`), including explicit inserts
that pre-empt ffmpeg's auto-inserted default-quality conversions (before
`arnndn`, after each loudnorm, both branches of the intro filtergraph).
Stock swr options only — the Docker image's Debian ffmpeg doesn't ship soxr.

**Measured result** (same raw chunk through the full chain incl. final MP3
encode): artifact band in sibilant frames −95 dB → **−136 dB** (below the
noise floor, zero spectral prominence); loudness/TP/LRA byte-identical to the
old chain. CPU cost negligible.

**Rule going forward**: any new `aresample`, `-ar`, or filter that forces a
sample rate (check the filter's supported rates — `arnndn`=48k,
`loudnorm`=192k internally) must use or be followed by `RESAMPLE_HQ`
parameters, or it will re-introduce imaging.

## Diagnostic toolkit

Throwaway scripts (`npx dotenvx run -- bun …` for anything needing `.env`;
plain `uv run --with numpy --with scipy python …` for analysis). Patterns that
paid off:

Spectrogram overview (artifacts above the ~11 kHz Higgs shelf jump out):

```bash
ffmpeg -y -i episode.mp3 -lavfi \
  "showspectrumpic=s=3600x1024:legend=1:scale=log:fscale=lin:stop=20000" spec.png
```

Isolate and amplify the high band to make a suspected artifact listenable
(ground truth for "is this what I'm hearing?"):

```bash
ffmpeg -y -ss 12 -i episode.mp3 -af \
  "highpass=f=12000,highpass=f=12000,highpass=f=12000,highpass=f=12000,volume=30dB" \
  artifact-isolated.wav
```

Measure how loud the artifact actually is (skip the full-band intro jingle):

```bash
ffmpeg -y -ss 12 -i episode.mp3 -af \
  "highpass=f=12000,highpass=f=12000,highpass=f=12000,highpass=f=12000" -f null - \
  # then astats: Peak level ≳ -45 dBFS = audible on headphones
```

Fetch a raw chunk from the Higgs server to separate model artifacts from
chain artifacts (sibilance-heavy text maximizes the high band):

```bash
curl -s -m 240 -X POST http://<mlx-host>:8000/v1/audio/speech \
  -H 'Content-Type: application/json' -d '{
    "model": "bosonai/higgs-audio-v3-tts-4b",
    "input": "The scientists insisted that the specifics stay strictly secret across successive sessions.",
    "gender": "male", "speed": 0.9, "max_tokens": 3000, "response_format": "mp3"
  }' -o raw_chunk.mp3
```

For narrowband suspects, the decisive test is **band-energy correlation over
time** (STFT, compare the suspect band's envelope against sibilance / speech /
control bands) plus **mirror arithmetic**: an image of source content at `f`
created when upsampling from rate `r` lands at `r − f`. If the suspect
frequency's mirror lands in a band that correlates with it, it's resampler
imaging; if it correlates with nothing, look at noise-shaping/codec artifacts
instead.

## Known next steps

- **~10.5 kHz gentle lowpass (Higgs only)**: the raw model output has real
  vocoder fizz at 10.0–10.7 kHz, just under its band edge. It's in-band
  content, partially masked by sibilance, and was NOT the ring complaint — so
  it's deliberately untouched. If a residual "tinge" is still audible after
  the imaging fix, a gentle lowpass around 10.5 kHz on the denoise path is
  the next lever. Costs genuine sibilance sparkle; A/B before shipping.
- **Whisper round-trip verify** (from the design invariants): the length-bounds
  check catches catastrophic truncation/runaway only. If subtle content drops
  surface, transcribe each chunk and diff against input text.
- **MP3 96k pre-echo**: not currently audible, but if a "smear before
  transients" complaint ever comes in, suspect the encoder before the chain —
  test by encoding a mastered WAV at 128k/V2 and comparing.
