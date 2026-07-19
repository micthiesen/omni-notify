import { baseConfigSchema, logConfig, stringBoolean } from "@micthiesen/mitools/config";
import { z } from "zod";

export type ChannelEntry = { username: string; displayName: string };

const channelList = z
  .string()
  .optional()
  .transform((val): ChannelEntry[] => {
    if (!val) return [];
    return val.split(",").map((entry) => {
      const [username, displayName] = entry.split(":");
      return { username, displayName: displayName ?? username };
    });
  });

const optionalPositiveInt = z.preprocess(
  (value) => (value === "" ? undefined : value),
  z.coerce.number().int().positive().optional(),
);

const configSchema = baseConfigSchema
  .extend({
    YT_CHANNEL_NAMES: channelList,
    TWITCH_CHANNEL_NAMES: channelList,
    KICK_CHANNEL_NAMES: channelList,
    KICK_CLIENT_ID: z.string().optional(),
    KICK_CLIENT_SECRET: z.string().optional(),
    OFFLINE_NOTIFICATIONS: z
      .string()
      .optional()
      .default("true")
      .transform(stringBoolean),
    PUSHOVER_LIVE_TOKEN: z.string().optional(),
    PUSHOVER_CALENDAR_TOKEN: z.string().optional(),
    PUSHOVER_BRIEFING_TOKEN: z.string().optional(),
    BRIEFING_MODEL: z.string().optional(),
    GOOGLE_GENERATIVE_AI_API_KEY: z.string().optional(),
    ANTHROPIC_API_KEY: z.string().optional(),
    OPENAI_API_KEY: z.string().optional(),
    TAVILY_API_KEY: z.string().optional(),
    LOGS_PATH: z.string().optional(),
    CHANNELS_CONFIG_PATH: z.string().optional(),
    BRIEFINGS_PATH: z.string().optional(),
    FASTMAIL_API_TOKEN: z.string().optional(),
    FASTMAIL_APP_PASSWORD: z.string().optional(),
    FASTMAIL_USERNAME: z.string().optional(),
    PARCEL_API_KEY: z.string().optional(),
    EXTRACTION_MODEL: z.string().optional(),
    /** Calendar extraction gets its own (stronger) model; falls back in registry. */
    CALENDAR_EXTRACTION_MODEL: z.string().optional(),
    /** Cheap shared email triage classifier (parcel + calendar relevance). */
    TRIAGE_MODEL: z.string().optional(),
    FASTMAIL_CALENDAR_ID: z.string().optional(),
    TMDB_API_KEY: z.string().optional(),
    RECS_SHORTLIST_MODEL: z.string().optional(),
    RECS_SELECTION_MODEL: z.string().optional(),
    TASTE_REFLECTION_MODEL: z.string().optional(),
    TASTE_REFLECTION_SCHEDULE: z.string().optional().default("0 0 4 * * 0"),
    RECS_SCHEDULE: z.string().optional().default("0 0 17 * * 1,3,5"),
    RECS_PUBLIC_URL: z.string().optional().default("http://omni.boris"),
    PUSHOVER_RECS_TOKEN: z.string().optional(),
    PODCAST_RECS_SCHEDULE: z.string().optional().default("0 0 11 * * 1,3,5"),
    PODCAST_TASTE_PATH: z.string().optional(),
    PODCAST_TASTE_REFLECTION_MODEL: z.string().optional(),
    PODCAST_TASTE_REFLECTION_SCHEDULE: z.string().optional().default("0 0 5 * * 0"),
    PUSHOVER_PODCAST_TOKEN: z.string().optional(),
    CASTRO_ACCESS_ID: z.string().uuid().optional(),
    CASTRO_SECRET_KEY: z.string().optional(),
    PODCASTINDEX_KEY: z.string().optional(),
    PODCASTINDEX_SECRET: z.string().optional(),
    /** Max followed voices person-searched per run (rotates across runs). */
    PODCAST_VOICE_ROTATION_MAX: optionalPositiveInt.default(12),
    /** Cap on Tier-1 guest-appearance picks per run (bursts like book tours). */
    PODCAST_MAX_GUEST_PICKS: optionalPositiveInt.default(6),
    PLEX_URL: z.string().optional(),
    PLEX_TOKEN: z.string().optional(),
    PLEX_ACCOUNT_ID: optionalPositiveInt,
    RADARR_URL: z.string().optional(),
    RADARR_API_KEY: z.string().optional(),
    RADARR_ROOT_FOLDER_PATH: z.string().optional(),
    RADARR_QUALITY_PROFILE_ID: optionalPositiveInt,
    SONARR_URL: z.string().optional(),
    SONARR_API_KEY: z.string().optional(),
    SONARR_ROOT_FOLDER_PATH: z.string().optional(),
    SONARR_QUALITY_PROFILE_ID: optionalPositiveInt,
    TZ: z.string().optional().default("America/Vancouver"),
    SMTP_HOST: z.string().optional(),
    SMTP_PORT: z.coerce.number().optional().default(587),
    SMTP_USER: z.string().optional(),
    SMTP_PASS: z.string().optional(),
    EMAIL_FROM: z.string().optional(),
    LOGS_EMAIL_TO: z.string().optional(),
    WHISKER_CREDENTIALS: z
      .string()
      .optional()
      .transform((s) => {
        if (!s) return undefined;
        const [email, ...rest] = s.split(":");
        const password = rest.join(":");
        if (!email || !password) {
          throw new Error("WHISKER_CREDENTIALS must be email:password");
        }
        return { email, password };
      }),
    FRONTEND_PORT: z.coerce.number().optional().default(3000),
    /** Enables PressPods (article → podcast) and authenticates its public routes. */
    PRESSPODS_AUTH_TOKEN: z.string().optional(),
    /**
     * Public origin for RSS enclosure URLs (e.g. the nginx-exposed host).
     * When unset, the origin is derived from each request's forwarded headers.
     */
    PRESSPODS_PUBLIC_URL: z
      .string()
      .optional()
      .transform((s) => s?.replace(/\/+$/, "")),
    /** Episode MP3 directory; defaults to press-pods-audio next to the DB. */
    PRESSPODS_AUDIO_DIR: z.string().optional(),
    PRESSPODS_METADATA_MODEL: z.string().optional(),
    PRESSPODS_CLEANING_MODEL: z.string().optional(),
    MISTRAL_API_KEY: z.string().optional(),
    JINA_API_KEY: z.string().optional(),
    PUSHOVER_PRESSPODS_TOKEN: z.string().optional(),
  })
  .transform((c) => ({
    ...c,
    PUSHOVER_LIVE_TOKEN: c.PUSHOVER_LIVE_TOKEN ?? c.PUSHOVER_TOKEN,
    PUSHOVER_BRIEFING_TOKEN: c.PUSHOVER_BRIEFING_TOKEN ?? c.PUSHOVER_TOKEN,
    PUSHOVER_CALENDAR_TOKEN: c.PUSHOVER_CALENDAR_TOKEN ?? c.PUSHOVER_TOKEN,
    PUSHOVER_RECS_TOKEN: c.PUSHOVER_RECS_TOKEN ?? c.PUSHOVER_TOKEN,
    PUSHOVER_PODCAST_TOKEN: c.PUSHOVER_PODCAST_TOKEN ?? c.PUSHOVER_TOKEN,
    PUSHOVER_PRESSPODS_TOKEN: c.PUSHOVER_PRESSPODS_TOKEN ?? c.PUSHOVER_TOKEN,
  }));

export type Config = z.infer<typeof configSchema>;

const config = configSchema.parse(process.env);
logConfig(config);

export default config;
