# Podcast recommendations: design

A sibling of the media-recommendation system, specialized for episode-level,
freshness-critical picks. Enabled by `PODCAST_TASTE_PATH` (the taste profile
doubles as the feature flag). Runs Mon/Thu 11am by default (`PODCAST_RECS_SCHEDULE`).

## The model: people-first, not topic-first

The highest-value recommendation is **an episode where a voice the listener
follows appears as a guest somewhere they don't already listen** — not a
generically popular episode about a topic. The pipeline is two-tier:

- **Tier 1 — guest appearances (the point).** For a rotated batch of "voices"
  (people named in the taste profile's `## Voices` section), find recent
  episodes featuring them as guests. Default-include (following the person is
  the signal); a light model gate only drops off-taste/trivial ones. Capped at
  `PODCAST_MAX_GUEST_PICKS` (default 6) so a press-tour week can surface several.
- **Tier 2 — topic/drama (fill).** The original multi-angle web-search discovery
  → cheap shortlist → strong-model one-pick selection. Conservative (0–2), and
  suppressed entirely once Tier 1 has delivered ≥3, to avoid flooding.

Everything downstream (RSS-verified recency, subscribed-show exclusion,
per-episode/30-day-show cooldowns, the pending→enqueue→notify Castro commit) is
shared across both tiers.

## Guest discovery sources (`guests.ts`)

Per voice, cheapest-first:

1. **Podcast Index `search/byperson`** (`podcastindex/`, free key
   `PODCASTINDEX_KEY`/`PODCASTINDEX_SECRET`). Structured, reads RSS
   `podcast:person` tags, returns fully-resolved episodes (feed URL, guid,
   enclosure/media URL, verified `datePublished`) — mapped straight to a
   candidate with no iTunes/RSS round-trip. Coverage depends on feeds tagging
   people (partial but growing). byperson returns all-time matches, so we
   filter to the 7-day recency window ourselves.
2. **Tavily person-search fallback** — only when Podcast Index has no recent hit
   for that voice. Covers non-podcasters (e.g. a streamer) and untagged feeds.
   A cheap model extracts guest episodes from the results; they resolve through
   the normal iTunes/RSS path. Keeping this second bounds the paid Tavily calls
   to what the free source missed.

Voices are rotated `PODCAST_VOICE_ROTATION_MAX` (default 12) per run via a
persisted cursor (`nextVoiceBatch`), so the whole list is covered over
successive runs without a large per-run Tavily bill.

The `## Voices` list lives in the taste markdown (single source of truth: the
discovery code parses it via `voices.ts`, and the model sees it too). Prefer
full, distinctive names — bare single-word handles risk missed or false matches.

## Drama / debate

The listener likes genuine drama, beefs, and contentious debates (Blocked and
Reported is the anchor) but NOT sensemaker-guru grift or rage-farming. This is a
prompt-side concern: the taste profile states it as a positive with that anchor,
and a Tier-2 discovery query targets it. The filters don't special-case it.

## Taste reflection (`reflection/`)

The taste digest fed to every model call has a fourth section beyond the seed
profile, subscriptions, and explicit feedback: a versioned reflective profile
built weekly by `PodcastTasteReflectionTask` (Sunday 5am, ±5min Castro jitter).
It is a deliberate sibling of `recommendations/taste/`: derive append-only
evidence (Castro listen history over the full 180-day window, plus delivered
recommendation outcomes and feedback), fingerprint it to skip no-op model calls,
run a draft pass then a skeptical critic pass, and validate claims in code
before persisting a checkpoint. Independence is counted in *shows*: stable,
conditional, and saturation claims need at least two distinct shows behind
them, while one explicit not-for-me can carry an aversion. Listens count as
taste-bearing only when finished (≥80%), starred, or reported without
completion data (the episode's duration was unknown, so no fraction could be
computed and the bare playback event is taken at face value). The latest profile is served at
`GET /api/podcast-recommendations/taste-profile` and rendered as the "Taste
brain" on the Podcasts page.

## Deferred: self-maintained feeder-feed scan

We deliberately did NOT build a curated "feeder feeds" list (a set of interview
shows to scan for guest names), because a hand-maintained feed list is a
maintenance trap and caps coverage to shows you remembered to add. Person-search
(Podcast Index + Tavily) covers "anywhere" without that upkeep.

If recall ever disappoints, the additive upgrade is a **self-learning** feeder
scan that needs no manual list: remember every feed on which a followed voice
has previously been found (from past Tier-1 hits), and each run pull those feeds'
recent episodes and name-match the voices in title/description. It self-builds
from real hits, stays free (RSS only), and is high-precision (interview shows
name guests in the episode metadata). Watch out for mononym false positives
("Destiny") — restrict name-matching to full/distinctive names. Podchaser's
structured credit graph would also slot in here as a paid 4th source, but its
free tier is gone ($30/mo minimum as of 2026), so it's not worth it for now.
