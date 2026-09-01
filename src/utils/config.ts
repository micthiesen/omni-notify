import { logConfig } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { Context, Data, Effect, Layer, Schema } from "effect";
import { validateMcpTokenConfiguration } from "../mcp/auth.js";

const optionalString = Schema.optional(Schema.String);
const defaultString = (value: string) =>
  Schema.optionalWith(Schema.String, { default: () => value });

const booleanFromString = Schema.transform(
  Schema.String.pipe(
    Schema.filter((value) => ["true", "false"].includes(value.toLowerCase()), {
      message: () => 'Expected "true" or "false"',
    }),
  ),
  Schema.Boolean,
  {
    decode: (value) => value.toLowerCase() === "true",
    encode: String,
  },
);

const finiteNumberFromString = Schema.NumberFromString.pipe(Schema.finite());
const positiveIntegerFromString = finiteNumberFromString.pipe(
  Schema.int(),
  Schema.positive(),
);
const emptyStringAsUndefined = Schema.transform(Schema.Literal(""), Schema.Undefined, {
  decode: () => undefined,
  encode: () => "" as const,
});
const emptyStringAsNumber = <const Value extends number>(value: Value) =>
  Schema.transform(Schema.Literal(""), Schema.Literal(value), {
    decode: () => value,
    encode: () => "" as const,
  });
const coercedFiniteNumber = Schema.Union(
  finiteNumberFromString,
  emptyStringAsNumber(0),
);
const positiveIntegerWithDefault = <const Value extends number>(value: Value) =>
  Schema.Union(positiveIntegerFromString, emptyStringAsNumber(value));
const nonNegativeNumberWithDefault = <const Value extends number>(value: Value) =>
  Schema.Union(
    finiteNumberFromString.pipe(Schema.nonNegative()),
    emptyStringAsNumber(value),
  );
const optionalPositiveInteger = Schema.optional(
  Schema.Union(positiveIntegerFromString, emptyStringAsUndefined),
);

const trimmedOrigin = Schema.transform(Schema.String, Schema.String, {
  decode: (value) => value.replace(/\/+$/, ""),
  encode: (value) => value,
});

const urlString = Schema.String.pipe(
  Schema.filter(
    (value) => {
      try {
        new URL(value);
        return true;
      } catch {
        return false;
      }
    },
    { message: () => "Expected a valid URL" },
  ),
);

const trimmedUrlString = Schema.transform(urlString, Schema.String, {
  decode: (value) => value.replace(/\/+$/, ""),
  encode: (value) => value,
});

const ippUrlString = urlString.pipe(
  Schema.filter((value) => ["ipp:", "ipps:"].includes(new URL(value).protocol), {
    message: () => "PRINTER_IPP_URL must use ipp:// or ipps://",
  }),
);

const whiskerCredentials = Schema.transform(
  Schema.String.pipe(
    Schema.filter(
      (value) => {
        const separator = value.indexOf(":");
        return separator > 0 && separator < value.length - 1;
      },
      { message: () => "WHISKER_CREDENTIALS must be email:password" },
    ),
  ),
  Schema.Struct({ email: Schema.String, password: Schema.String }),
  {
    decode: (value) => {
      const separator = value.indexOf(":");
      return {
        email: value.slice(0, separator),
        password: value.slice(separator + 1),
      };
    },
    encode: ({ email, password }) => `${email}:${password}`,
  },
);

const rawConfigSchema = Schema.Struct({
  LOG_LEVEL: Schema.optionalWith(Schema.Enums(LogLevel), {
    default: () => LogLevel.INFO,
  }),
  PUSHOVER_USER: optionalString,
  PUSHOVER_TOKEN: optionalString,
  DOCKERIZED: Schema.optionalWith(booleanFromString, { default: () => false }),
  DB_NAME: defaultString("docstore.db"),
  KICK_CLIENT_ID: optionalString,
  KICK_CLIENT_SECRET: optionalString,
  OFFLINE_NOTIFICATIONS: Schema.optionalWith(booleanFromString, {
    default: () => true,
  }),
  PUSHOVER_LIVE_TOKEN: optionalString,
  LIVESTREAM_INTELLIGENCE_ENABLED: Schema.optionalWith(booleanFromString, {
    default: () => false,
  }),
  LIVESTREAM_INTELLIGENCE_MODEL: Schema.optional(Schema.Literal("openai:gpt-5.6-luna")),
  LIVESTREAM_MONTHLY_BUDGET_USD: Schema.optionalWith(
    nonNegativeNumberWithDefault(3).pipe(Schema.lessThanOrEqualTo(10)),
    { default: () => 3 },
  ),
  LIVESTREAM_MODEL_DIR: defaultString("/app/assets/livestream-intelligence/models"),
  LIVESTREAM_DESTINY_VOICEPRINT_PATH: optionalString,
  LIVESTREAM_DESTINY_SPEAKER_THRESHOLD: Schema.optionalWith(
    coercedFiniteNumber.pipe(Schema.between(0, 1)),
    { default: () => 0.62 },
  ),
  LIVESTREAM_MAX_VOICE_TARGETS: Schema.optionalWith(positiveIntegerWithDefault(3), {
    default: () => 3,
  }),
  LIVESTREAM_VOICE_SAMPLE_SECONDS: Schema.optionalWith(positiveIntegerWithDefault(18), {
    default: () => 18,
  }),
  LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS: Schema.optionalWith(
    positiveIntegerWithDefault(45),
    { default: () => 45 },
  ),
  LIVESTREAM_SUMMARY_SAMPLE_SECONDS: Schema.optionalWith(
    positiveIntegerWithDefault(75),
    { default: () => 75 },
  ),
  LIVESTREAM_SUMMARY_INTERVAL_SECONDS: Schema.optionalWith(
    positiveIntegerWithDefault(480),
    { default: () => 480 },
  ),
  PUSHOVER_CALENDAR_TOKEN: optionalString,
  PUSHOVER_BRIEFING_TOKEN: optionalString,
  PUSHOVER_WORKSPACE_TOKEN: optionalString,
  BRIEFING_MODEL: optionalString,
  WORKSPACE_MODEL: optionalString,
  WORKSPACE_SCHEDULE: defaultString("0 0 9 * * 0"),
  WORKSPACES_PUBLIC_URL: Schema.optionalWith(trimmedOrigin, {
    default: () => "http://omni.boris",
  }),
  GOOGLE_GENERATIVE_AI_API_KEY: optionalString,
  ANTHROPIC_API_KEY: optionalString,
  OPENAI_API_KEY: optionalString,
  TAVILY_API_KEY: optionalString,
  LOGS_PATH: optionalString,
  CHANNELS_CONFIG_PATH: optionalString,
  BRIEFINGS_PATH: optionalString,
  EMAIL_TRANSPORT: Schema.optional(Schema.Literal("fastmail", "icloud")),
  CALDAV_PROVIDER: Schema.optional(Schema.Literal("fastmail", "icloud")),
  FASTMAIL_API_TOKEN: optionalString,
  FASTMAIL_APP_PASSWORD: optionalString,
  FASTMAIL_USERNAME: optionalString,
  ICLOUD_USERNAME: optionalString,
  ICLOUD_APP_PASSWORD: optionalString,
  ICLOUD_CALENDAR_NAME: optionalString,
  ICLOUD_CALENDAR_URL: optionalString,
  EMAIL_SELF_ADDRESS: optionalString,
  PARCEL_API_KEY: optionalString,
  EXTRACTION_MODEL: optionalString,
  CALENDAR_EXTRACTION_MODEL: optionalString,
  TRIAGE_MODEL: optionalString,
  FASTMAIL_CALENDAR_ID: optionalString,
  TMDB_API_KEY: optionalString,
  RECS_SHORTLIST_MODEL: optionalString,
  RECS_SELECTION_MODEL: optionalString,
  TASTE_REFLECTION_MODEL: optionalString,
  TASTE_REFLECTION_SCHEDULE: defaultString("0 0 4 * * 0"),
  RECS_SCHEDULE: defaultString("0 0 17 * * 1,3,5"),
  RECS_PUBLIC_URL: defaultString("http://omni.boris"),
  PUSHOVER_RECS_TOKEN: optionalString,
  PODCAST_RECS_SCHEDULE: defaultString("0 0 11 * * 1,3,5"),
  PODCAST_TASTE_PATH: optionalString,
  PODCAST_TASTE_REFLECTION_MODEL: optionalString,
  PODCAST_TASTE_REFLECTION_SCHEDULE: defaultString("0 0 5 * * 0"),
  PUSHOVER_PODCAST_TOKEN: optionalString,
  CASTRO_ACCESS_ID: Schema.optional(Schema.UUID),
  CASTRO_SECRET_KEY: optionalString,
  PODCASTINDEX_KEY: optionalString,
  PODCASTINDEX_SECRET: optionalString,
  PODCAST_VOICE_ROTATION_MAX: Schema.optionalWith(positiveIntegerWithDefault(12), {
    default: () => 12,
  }),
  PODCAST_MAX_GUEST_PICKS: Schema.optionalWith(positiveIntegerWithDefault(6), {
    default: () => 6,
  }),
  PLEX_URL: optionalString,
  PLEX_TOKEN: optionalString,
  PLEX_ACCOUNT_ID: optionalPositiveInteger,
  RADARR_URL: optionalString,
  RADARR_API_KEY: optionalString,
  RADARR_ROOT_FOLDER_PATH: optionalString,
  RADARR_QUALITY_PROFILE_ID: optionalPositiveInteger,
  SONARR_URL: optionalString,
  SONARR_API_KEY: optionalString,
  SONARR_ROOT_FOLDER_PATH: optionalString,
  SONARR_QUALITY_PROFILE_ID: optionalPositiveInteger,
  TZ: defaultString("America/Vancouver"),
  SMTP_HOST: optionalString,
  SMTP_PORT: Schema.optionalWith(coercedFiniteNumber, { default: () => 587 }),
  SMTP_USER: optionalString,
  SMTP_PASS: optionalString,
  EMAIL_FROM: optionalString,
  LOGS_EMAIL_TO: optionalString,
  WHISKER_CREDENTIALS: Schema.optional(whiskerCredentials),
  FRONTEND_PORT: Schema.optionalWith(coercedFiniteNumber, {
    default: () => 3000,
  }),
  OMNI_MCP_TOKEN: optionalString,
  PRINTER_IPP_URL: Schema.optional(ippUrlString),
  PRESSPODS_AUTH_TOKEN: optionalString,
  PRESSPODS_PUBLIC_URL: Schema.optional(trimmedOrigin),
  PRESSPODS_AUDIO_DIR: optionalString,
  PRESSPODS_METADATA_MODEL: optionalString,
  PRESSPODS_CLEANING_MODEL: optionalString,
  PRESSPODS_TTS_PROVIDER: Schema.optionalWith(Schema.Literal("higgs", "elevenlabs"), {
    default: () => "higgs" as const,
  }),
  PRESSPODS_TTS_URL: Schema.optional(trimmedOrigin),
  PRESSPODS_TTS_MODEL: optionalString,
  PRESSPODS_STT_URL: Schema.optional(trimmedOrigin),
  PRESSPODS_STT_MODEL: optionalString,
  PRESSPODS_HIGGS_REF_AUDIO: optionalString,
  PRESSPODS_HIGGS_REF_TEXT: optionalString,
  ELEVENLABS_API_KEY: optionalString,
  ELEVENLABS_VOICE_MALE: optionalString,
  ELEVENLABS_VOICE_FEMALE: optionalString,
  MISTRAL_API_KEY: optionalString,
  JINA_API_KEY: optionalString,
  PUSHOVER_PRESSPODS_TOKEN: optionalString,
  IOS_CONTROL_AUTH_TOKEN: Schema.optional(Schema.String.pipe(Schema.minLength(24))),
  IOS_CONTROL_HOME_URL: Schema.optionalWith(trimmedUrlString, {
    default: () => "http://omni.boris",
  }),
  IOS_CONTROL_APNS_TEAM_ID: optionalString,
  IOS_CONTROL_APNS_KEY_ID: optionalString,
  IOS_CONTROL_APNS_KEY_PATH: optionalString,
  IOS_CONTROL_BUNDLE_ID: defaultString("com.micthiesen.OmniLive"),
});

type RawConfig = Schema.Schema.Type<typeof rawConfigSchema>;

const deriveConfig = (raw: RawConfig) => {
  const emailTransport =
    raw.EMAIL_TRANSPORT ??
    (raw.FASTMAIL_API_TOKEN
      ? ("fastmail" as const)
      : raw.ICLOUD_USERNAME && raw.ICLOUD_APP_PASSWORD
        ? ("icloud" as const)
        : undefined);

  return {
    ...raw,
    EMAIL_TRANSPORT: emailTransport,
    EMAIL_SELF_ADDRESS:
      raw.EMAIL_SELF_ADDRESS ??
      (emailTransport === "icloud"
        ? raw.ICLOUD_USERNAME
        : emailTransport === "fastmail"
          ? raw.FASTMAIL_USERNAME
          : (raw.ICLOUD_USERNAME ?? raw.FASTMAIL_USERNAME)),
    PUSHOVER_LIVE_TOKEN: raw.PUSHOVER_LIVE_TOKEN ?? raw.PUSHOVER_TOKEN,
    PUSHOVER_BRIEFING_TOKEN: raw.PUSHOVER_BRIEFING_TOKEN ?? raw.PUSHOVER_TOKEN,
    PUSHOVER_WORKSPACE_TOKEN: raw.PUSHOVER_WORKSPACE_TOKEN ?? raw.PUSHOVER_TOKEN,
    PUSHOVER_CALENDAR_TOKEN: raw.PUSHOVER_CALENDAR_TOKEN ?? raw.PUSHOVER_TOKEN,
    PUSHOVER_RECS_TOKEN: raw.PUSHOVER_RECS_TOKEN ?? raw.PUSHOVER_TOKEN,
    PUSHOVER_PODCAST_TOKEN: raw.PUSHOVER_PODCAST_TOKEN ?? raw.PUSHOVER_TOKEN,
    PUSHOVER_PRESSPODS_TOKEN: raw.PUSHOVER_PRESSPODS_TOKEN ?? raw.PUSHOVER_TOKEN,
  };
};

export type Config = ReturnType<typeof deriveConfig>;

export class ConfigLoadError extends Data.TaggedError("ConfigLoadError")<{
  readonly cause: unknown;
}> {}

const privateConfigKeys = [
  "PUSHOVER_USER",
  "FASTMAIL_USERNAME",
  "ICLOUD_USERNAME",
  "EMAIL_SELF_ADDRESS",
  "SMTP_USER",
  "EMAIL_FROM",
  "LOGS_EMAIL_TO",
] as const;

/** Decode an explicit environment without reading or mutating process globals. */
export const loadConfigEffect = (
  environment: Readonly<Record<string, string | undefined>> = process.env,
): Effect.Effect<Config, ConfigLoadError> =>
  Schema.decodeUnknown(rawConfigSchema)(environment).pipe(
    Effect.map(deriveConfig),
    Effect.tap((parsed) =>
      Effect.try({
        try: () => {
          validateMcpTokenConfiguration(parsed.OMNI_MCP_TOKEN, parsed.DOCKERIZED);
          logConfig(parsed, [...privateConfigKeys]);
        },
        catch: (cause) => new ConfigLoadError({ cause }),
      }),
    ),
    Effect.mapError((cause) =>
      cause instanceof ConfigLoadError ? cause : new ConfigLoadError({ cause }),
    ),
  );

/** Injectable configuration service for Effect-native workflows. */
export class AppConfig extends Context.Tag("AppConfig")<AppConfig, Config>() {}

export const makeAppConfigLayer = (
  environment: Readonly<Record<string, string | undefined>> = process.env,
): Layer.Layer<AppConfig, ConfigLoadError> =>
  Layer.effect(AppConfig, loadConfigEffect(environment));

const config = Effect.runSync(loadConfigEffect());

export default config;
