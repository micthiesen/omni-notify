import { logConfig } from "@micthiesen/mitools/config";
import { LogLevel } from "@micthiesen/mitools/logging";
import { Data, Effect, Schema, SchemaGetter } from "effect";
import { validateMcpTokenConfiguration } from "../mcp/auth.js";

const optionalString = Schema.optional(Schema.String);
const defaultString = (value: string) =>
  Schema.String.pipe(Schema.withDecodingDefaultType(Effect.succeed(value)));

const booleanFromString = Schema.String.pipe(
  Schema.check(
    Schema.makeFilter((value) => ["true", "false"].includes(value.toLowerCase()), {
      message: 'Expected "true" or "false"',
    }),
  ),
  Schema.decodeTo(Schema.Boolean, {
    decode: SchemaGetter.transform((value) => value.toLowerCase() === "true"),
    encode: SchemaGetter.transform(String),
  }),
);

const finiteNumberFromString = Schema.FiniteFromString;
const positiveIntegerFromString = finiteNumberFromString.pipe(
  Schema.check(Schema.isInt(), Schema.isGreaterThan(0)),
);
const emptyStringAsUndefined = Schema.Literal("").pipe(
  Schema.decodeTo(Schema.Undefined, {
    decode: SchemaGetter.transform(() => undefined),
    encode: SchemaGetter.transform(() => "" as const),
  }),
);
const emptyStringAsNumber = <const Value extends number>(value: Value) =>
  Schema.Literal("").pipe(
    Schema.decodeTo(Schema.Literal(value), {
      decode: SchemaGetter.transform(() => value),
      encode: SchemaGetter.transform(() => "" as const),
    }),
  );
const coercedFiniteNumber = Schema.Union([
  finiteNumberFromString,
  emptyStringAsNumber(0),
]);
const positiveIntegerWithDefault = <const Value extends number>(value: Value) =>
  Schema.Union([positiveIntegerFromString, emptyStringAsNumber(value)]);
const nonNegativeNumberWithDefault = <const Value extends number>(value: Value) =>
  Schema.Union([
    emptyStringAsNumber(value),
    finiteNumberFromString.pipe(Schema.check(Schema.isGreaterThanOrEqualTo(0))),
  ]);
const optionalPositiveInteger = Schema.optional(
  Schema.Union([positiveIntegerFromString, emptyStringAsUndefined]),
);

const trimmedOrigin = Schema.String.pipe(
  Schema.decode({
    decode: SchemaGetter.transform((value) => value.replace(/\/+$/, "")),
    encode: SchemaGetter.transform((value) => value),
  }),
);

const urlString = Schema.String.pipe(
  Schema.check(
    Schema.makeFilter(
      (value) => {
        try {
          new URL(value);
          return true;
        } catch {
          return false;
        }
      },
      { message: "Expected a valid URL" },
    ),
  ),
);

const trimmedUrlString = urlString.pipe(
  Schema.decode({
    decode: SchemaGetter.transform((value) => value.replace(/\/+$/, "")),
    encode: SchemaGetter.transform((value) => value),
  }),
);

const ippUrlString = urlString.pipe(
  Schema.check(
    Schema.makeFilter((value) => ["ipp:", "ipps:"].includes(new URL(value).protocol), {
      message: "PRINTER_IPP_URL must use ipp:// or ipps://",
    }),
  ),
);

const whiskerCredentials = Schema.String.pipe(
  Schema.check(
    Schema.makeFilter(
      (value) => {
        const separator = value.indexOf(":");
        return separator > 0 && separator < value.length - 1;
      },
      { message: "WHISKER_CREDENTIALS must be email:password" },
    ),
  ),
  Schema.decodeTo(Schema.Struct({ email: Schema.String, password: Schema.String }), {
    decode: SchemaGetter.transform((value) => {
      const separator = value.indexOf(":");
      return {
        email: value.slice(0, separator),
        password: value.slice(separator + 1),
      };
    }),
    encode: SchemaGetter.transform(({ email, password }) => `${email}:${password}`),
  }),
);

const rawConfigSchema = Schema.Struct({
  LOG_LEVEL: Schema.Enum(LogLevel).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(LogLevel.INFO)),
  ),
  PUSHOVER_USER: optionalString,
  PUSHOVER_TOKEN: optionalString,
  DOCKERIZED: booleanFromString.pipe(
    Schema.withDecodingDefaultType(Effect.succeed(false)),
  ),
  DB_NAME: defaultString("docstore.db"),
  KICK_CLIENT_ID: optionalString,
  KICK_CLIENT_SECRET: optionalString,
  OFFLINE_NOTIFICATIONS: booleanFromString.pipe(
    Schema.withDecodingDefaultType(Effect.succeed(true)),
  ),
  PUSHOVER_LIVE_TOKEN: optionalString,
  LIVESTREAM_INTELLIGENCE_ENABLED: booleanFromString.pipe(
    Schema.withDecodingDefaultType(Effect.succeed(false)),
  ),
  LIVESTREAM_INTELLIGENCE_MODEL: Schema.optional(Schema.Literal("openai:gpt-5.6-luna")),
  LIVESTREAM_MONTHLY_BUDGET_USD: nonNegativeNumberWithDefault(3).pipe(
    Schema.check(Schema.isLessThanOrEqualTo(10)),
    Schema.withDecodingDefaultType(Effect.succeed(3)),
  ),
  LIVESTREAM_MODEL_DIR: defaultString("/app/assets/livestream-intelligence/models"),
  LIVESTREAM_DESTINY_VOICEPRINT_PATH: optionalString,
  LIVESTREAM_DESTINY_SPEAKER_THRESHOLD: coercedFiniteNumber.pipe(
    Schema.check(Schema.isBetween({ minimum: 0, maximum: 1 })),
    Schema.withDecodingDefaultType(Effect.succeed(0.62)),
  ),
  LIVESTREAM_MAX_VOICE_TARGETS: positiveIntegerWithDefault(3).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(3)),
  ),
  LIVESTREAM_VOICE_SAMPLE_SECONDS: positiveIntegerWithDefault(18).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(18)),
  ),
  LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS: positiveIntegerWithDefault(45).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(45)),
  ),
  LIVESTREAM_SUMMARY_SAMPLE_SECONDS: positiveIntegerWithDefault(75).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(75)),
  ),
  LIVESTREAM_SUMMARY_INTERVAL_SECONDS: positiveIntegerWithDefault(480).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(480)),
  ),
  PUSHOVER_CALENDAR_TOKEN: optionalString,
  PUSHOVER_BRIEFING_TOKEN: optionalString,
  PUSHOVER_WORKSPACE_TOKEN: optionalString,
  BRIEFING_MODEL: optionalString,
  WORKSPACE_MODEL: optionalString,
  WORKSPACE_SCHEDULE: defaultString("0 0 9 * * 0"),
  WORKSPACES_PUBLIC_URL: trimmedOrigin.pipe(
    Schema.withDecodingDefaultType(Effect.succeed("http://omni.boris")),
  ),
  GOOGLE_GENERATIVE_AI_API_KEY: optionalString,
  ANTHROPIC_API_KEY: optionalString,
  OPENAI_API_KEY: optionalString,
  TAVILY_API_KEY: optionalString,
  LOGS_PATH: optionalString,
  CHANNELS_CONFIG_PATH: optionalString,
  BRIEFINGS_PATH: optionalString,
  ICLOUD_USERNAME: optionalString,
  ICLOUD_APP_PASSWORD: optionalString,
  ICLOUD_CALENDAR_NAME: optionalString,
  ICLOUD_CALENDAR_URL: optionalString,
  EMAIL_SELF_ADDRESS: optionalString,
  PARCEL_API_KEY: optionalString,
  EXTRACTION_MODEL: optionalString,
  CALENDAR_EXTRACTION_MODEL: optionalString,
  TRIAGE_MODEL: optionalString,
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
  CASTRO_ACCESS_ID: Schema.optional(Schema.String.pipe(Schema.check(Schema.isUUID()))),
  CASTRO_SECRET_KEY: optionalString,
  PODCASTINDEX_KEY: optionalString,
  PODCASTINDEX_SECRET: optionalString,
  PODCAST_VOICE_ROTATION_MAX: positiveIntegerWithDefault(12).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(12)),
  ),
  PODCAST_MAX_GUEST_PICKS: positiveIntegerWithDefault(6).pipe(
    Schema.withDecodingDefaultType(Effect.succeed(6)),
  ),
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
  SMTP_PORT: coercedFiniteNumber.pipe(
    Schema.withDecodingDefaultType(Effect.succeed(587)),
  ),
  SMTP_USER: optionalString,
  SMTP_PASS: optionalString,
  EMAIL_FROM: optionalString,
  LOGS_EMAIL_TO: optionalString,
  WHISKER_CREDENTIALS: Schema.optional(whiskerCredentials),
  FRONTEND_PORT: coercedFiniteNumber.pipe(
    Schema.withDecodingDefaultType(Effect.succeed(3000)),
  ),
  OMNI_MCP_TOKEN: optionalString,
  PRINTER_IPP_URL: Schema.optional(ippUrlString),
  PRESSPODS_AUTH_TOKEN: optionalString,
  PRESSPODS_PUBLIC_URL: Schema.optional(trimmedOrigin),
  PRESSPODS_AUDIO_DIR: optionalString,
  PRESSPODS_METADATA_MODEL: optionalString,
  PRESSPODS_CLEANING_MODEL: optionalString,
  PRESSPODS_TTS_PROVIDER: Schema.Literals(["higgs", "elevenlabs"]).pipe(
    Schema.withDecodingDefaultType(Effect.succeed("higgs" as const)),
  ),
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
  IOS_CONTROL_AUTH_TOKEN: Schema.optional(
    Schema.String.pipe(Schema.check(Schema.isMinLength(24))),
  ),
  IOS_CONTROL_HOME_URL: trimmedUrlString.pipe(
    Schema.withDecodingDefaultType(Effect.succeed("http://omni.boris")),
  ),
  IOS_CONTROL_APNS_TEAM_ID: optionalString,
  IOS_CONTROL_APNS_KEY_ID: optionalString,
  IOS_CONTROL_APNS_KEY_PATH: optionalString,
  IOS_CONTROL_BUNDLE_ID: defaultString("com.micthiesen.OmniLive"),
});

type RawConfig = Schema.Schema.Type<typeof rawConfigSchema>;

const deriveConfig = (raw: RawConfig) => {
  return {
    ...raw,
    EMAIL_SELF_ADDRESS: raw.EMAIL_SELF_ADDRESS ?? raw.ICLOUD_USERNAME,
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
  Schema.decodeUnknownEffect(rawConfigSchema)(environment).pipe(
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

function loadBootConfig(
  environment: Readonly<Record<string, string | undefined>>,
): Config {
  try {
    const parsed = deriveConfig(Schema.decodeUnknownSync(rawConfigSchema)(environment));
    validateMcpTokenConfiguration(parsed.OMNI_MCP_TOKEN, parsed.DOCKERIZED);
    logConfig(parsed, [...privateConfigKeys]);
    return parsed;
  } catch (cause) {
    throw new ConfigLoadError({ cause });
  }
}

// Module initialization is the process bootstrap adapter. Runtime workflows
// receive this immutable decoded value and never re-read process.env.
const config = loadBootConfig(process.env);

export default config;
