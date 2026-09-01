import { Effect } from "effect";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ConfigLoadError, loadConfigEffect } from "./config.js";

const load = (environment: Record<string, string | undefined> = {}) =>
  Effect.runSync(loadConfigEffect(environment));

describe("Effect application configuration", () => {
  beforeEach(() => {
    vi.spyOn(console, "log").mockImplementation(() => undefined);
  });

  it("preserves application and inherited defaults", () => {
    const config = load();

    expect(config).toMatchObject({
      LOG_LEVEL: "info",
      DOCKERIZED: false,
      DB_NAME: "docstore.db",
      OFFLINE_NOTIFICATIONS: true,
      LIVESTREAM_INTELLIGENCE_ENABLED: false,
      LIVESTREAM_MONTHLY_BUDGET_USD: 3,
      LIVESTREAM_DESTINY_SPEAKER_THRESHOLD: 0.62,
      LIVESTREAM_MAX_VOICE_TARGETS: 3,
      LIVESTREAM_VOICE_SAMPLE_SECONDS: 18,
      LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS: 45,
      LIVESTREAM_SUMMARY_SAMPLE_SECONDS: 75,
      LIVESTREAM_SUMMARY_INTERVAL_SECONDS: 480,
      WORKSPACE_SCHEDULE: "0 0 9 * * 0",
      WORKSPACES_PUBLIC_URL: "http://omni.boris",
      TASTE_REFLECTION_SCHEDULE: "0 0 4 * * 0",
      RECS_SCHEDULE: "0 0 17 * * 1,3,5",
      PODCAST_RECS_SCHEDULE: "0 0 11 * * 1,3,5",
      PODCAST_TASTE_REFLECTION_SCHEDULE: "0 0 5 * * 0",
      PODCAST_VOICE_ROTATION_MAX: 12,
      PODCAST_MAX_GUEST_PICKS: 6,
      TZ: "America/Vancouver",
      SMTP_PORT: 587,
      FRONTEND_PORT: 3000,
      PRESSPODS_TTS_PROVIDER: "higgs",
      IOS_CONTROL_HOME_URL: "http://omni.boris",
      IOS_CONTROL_BUNDLE_ID: "com.micthiesen.OmniLive",
    });
    expect(config.EMAIL_TRANSPORT).toBeUndefined();
  });

  it("decodes coercions, refinements, normalization, and Whisker credentials", () => {
    const config = load({
      OFFLINE_NOTIFICATIONS: "FALSE",
      LIVESTREAM_INTELLIGENCE_ENABLED: "true",
      LIVESTREAM_MONTHLY_BUDGET_USD: "4.5",
      LIVESTREAM_MAX_VOICE_TARGETS: "8",
      PLEX_ACCOUNT_ID: "",
      FRONTEND_PORT: "4100",
      WORKSPACES_PUBLIC_URL: "https://omni.example///",
      IOS_CONTROL_HOME_URL: "https://omni.example///",
      PRINTER_IPP_URL: "ipps://printer.example/ipp/print",
      CASTRO_ACCESS_ID: "550e8400-e29b-41d4-a716-446655440000",
      WHISKER_CREDENTIALS: "person@example.com:pass:with:colons",
    });

    expect(config.OFFLINE_NOTIFICATIONS).toBe(false);
    expect(config.LIVESTREAM_INTELLIGENCE_ENABLED).toBe(true);
    expect(config.LIVESTREAM_MONTHLY_BUDGET_USD).toBe(4.5);
    expect(config.LIVESTREAM_MAX_VOICE_TARGETS).toBe(8);
    expect(config.PLEX_ACCOUNT_ID).toBeUndefined();
    expect(config.FRONTEND_PORT).toBe(4100);
    expect(config.WORKSPACES_PUBLIC_URL).toBe("https://omni.example");
    expect(config.IOS_CONTROL_HOME_URL).toBe("https://omni.example");
    expect(config.WHISKER_CREDENTIALS).toEqual({
      email: "person@example.com",
      password: "pass:with:colons",
    });
  });

  it("preserves legacy empty-string coercion and default behavior", () => {
    const config = load({
      LIVESTREAM_MONTHLY_BUDGET_USD: "",
      LIVESTREAM_DESTINY_SPEAKER_THRESHOLD: "",
      LIVESTREAM_MAX_VOICE_TARGETS: "",
      SMTP_PORT: "",
      FRONTEND_PORT: "",
    });

    expect(config.LIVESTREAM_MONTHLY_BUDGET_USD).toBe(3);
    expect(config.LIVESTREAM_DESTINY_SPEAKER_THRESHOLD).toBe(0);
    expect(config.LIVESTREAM_MAX_VOICE_TARGETS).toBe(3);
    expect(config.SMTP_PORT).toBe(0);
    expect(config.FRONTEND_PORT).toBe(0);
  });

  it("derives transport, self address, and feature Pushover tokens", () => {
    const fastmail = load({
      FASTMAIL_API_TOKEN: "fastmail-api-token",
      FASTMAIL_USERNAME: "fastmail@example.com",
      PUSHOVER_TOKEN: "shared-pushover-token",
      PUSHOVER_RECS_TOKEN: "recommendations-token",
    });

    expect(fastmail.EMAIL_TRANSPORT).toBe("fastmail");
    expect(fastmail.EMAIL_SELF_ADDRESS).toBe("fastmail@example.com");
    expect(fastmail.PUSHOVER_LIVE_TOKEN).toBe("shared-pushover-token");
    expect(fastmail.PUSHOVER_CALENDAR_TOKEN).toBe("shared-pushover-token");
    expect(fastmail.PUSHOVER_RECS_TOKEN).toBe("recommendations-token");

    const icloud = load({
      ICLOUD_USERNAME: "icloud@example.com",
      ICLOUD_APP_PASSWORD: "app-password",
      FASTMAIL_USERNAME: "legacy@example.com",
    });
    expect(icloud.EMAIL_TRANSPORT).toBe("icloud");
    expect(icloud.EMAIL_SELF_ADDRESS).toBe("icloud@example.com");
  });

  it.each([
    { LIVESTREAM_MONTHLY_BUDGET_USD: "10.01" },
    { LIVESTREAM_DESTINY_SPEAKER_THRESHOLD: "-0.1" },
    { PODCAST_MAX_GUEST_PICKS: "0" },
    { CASTRO_ACCESS_ID: "not-a-uuid" },
    { PRINTER_IPP_URL: "https://printer.example/ipp/print" },
    { IOS_CONTROL_AUTH_TOKEN: "too-short" },
    { WHISKER_CREDENTIALS: "missing-password:" },
    { EMAIL_TRANSPORT: "smtp" },
    { DOCKERIZED: "prod" },
    { OFFLINE_NOTIFICATIONS: "yes" },
  ])("returns ConfigLoadError for invalid input %#", (environment) => {
    const error = Effect.runSync(Effect.flip(loadConfigEffect(environment)));
    expect(error).toBeInstanceOf(ConfigLoadError);
  });

  it("validates production MCP credentials after schema decoding", () => {
    const missing = Effect.runSync(
      Effect.flip(loadConfigEffect({ DOCKERIZED: "true" })),
    );
    expect(missing).toBeInstanceOf(ConfigLoadError);

    expect(() =>
      load({
        DOCKERIZED: "true",
        OMNI_MCP_TOKEN: "0123456789abcdefghijklmnopqrstuv",
      }),
    ).not.toThrow();
  });

  it("redacts secrets and identifying account fields when logging", () => {
    const log = vi.spyOn(console, "log").mockImplementation(() => undefined);
    load({
      PUSHOVER_USER: "private-user-id",
      PUSHOVER_TOKEN: "private-token",
      FASTMAIL_USERNAME: "private@example.com",
      FASTMAIL_API_TOKEN: "private-fastmail-token",
      WHISKER_CREDENTIALS: "private@example.com:private-password",
    });

    const output = JSON.stringify(log.mock.calls);
    expect(output).not.toContain("private-user-id");
    expect(output).not.toContain("private-token");
    expect(output).not.toContain("private@example.com");
    expect(output).not.toContain("private-password");
    expect(output).toContain("***");
  });
});
