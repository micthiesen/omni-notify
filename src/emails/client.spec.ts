import { describe, expect, it } from "vitest";
import { resolveComposeEmailConfiguration } from "./client.js";

const values = (
  overrides: Partial<Parameters<typeof resolveComposeEmailConfiguration>[0]> = {},
) => ({
  SMTP_HOST: "smtp.example.test",
  SMTP_PORT: 587,
  SMTP_USER: "smtp-user",
  SMTP_PASS: "smtp-pass",
  EMAIL_FROM: "sender@example.test",
  ICLOUD_USERNAME: "icloud-user",
  ICLOUD_APP_PASSWORD: "icloud-pass",
  ...overrides,
});

describe("compose SMTP configuration", () => {
  it("uses complete SMTP settings and selects TLS from the port", () => {
    expect(resolveComposeEmailConfiguration(values())).toEqual({
      host: "smtp.example.test",
      port: 587,
      user: "smtp-user",
      pass: "smtp-pass",
      from: "sender@example.test",
      secure: false,
      requireTLS: true,
      source: "smtp",
    });
    expect(resolveComposeEmailConfiguration(values({ SMTP_PORT: 465 }))).toMatchObject({
      secure: true,
      requireTLS: false,
    });
  });

  it("falls back to complete iCloud credentials and uses its username as the sender", () => {
    expect(
      resolveComposeEmailConfiguration(
        values({
          SMTP_HOST: "",
          SMTP_USER: "",
          SMTP_PASS: "",
          EMAIL_FROM: "",
        }),
      ),
    ).toEqual({
      host: "smtp.mail.me.com",
      port: 587,
      user: "icloud-user",
      pass: "icloud-pass",
      from: "icloud-user",
      secure: false,
      requireTLS: true,
      source: "icloud",
    });
  });

  it("rejects partial SMTP configuration and incomplete iCloud credentials", () => {
    expect(resolveComposeEmailConfiguration(values({ SMTP_PASS: "" }))).toBeUndefined();
    expect(
      resolveComposeEmailConfiguration(
        values({
          SMTP_HOST: "",
          SMTP_USER: "",
          SMTP_PASS: "",
          ICLOUD_APP_PASSWORD: "",
        }),
      ),
    ).toBeUndefined();
    expect(
      resolveComposeEmailConfiguration(
        values({ SMTP_HOST: "", SMTP_USER: "", SMTP_PASS: "", ICLOUD_USERNAME: "" }),
      ),
    ).toBeUndefined();
  });
});
