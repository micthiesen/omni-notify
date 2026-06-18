import { describe, expect, it } from "vitest";
import { filterCalendarCandidate } from "./keywords.js";

const make = (from: string, subject: string, textBody = "") => ({
  from,
  subject,
  textBody,
});

describe("filterCalendarCandidate", () => {
  describe("blacklisted senders", () => {
    it("rejects newsletters and social senders", () => {
      const result = filterCalendarCandidate(
        make("blockedandreported@substack.com", "Weekly Open Thread"),
      );
      expect(result.pass).toBe(false);
      expect(result.reason).toBe("blacklisted sender");
    });

    it("rejects food delivery senders even with calendar keywords present", () => {
      const result = filterCalendarCandidate(
        make("no-reply@doordash.com", "Order Confirmation for Michael from DashMart"),
      );
      expect(result.pass).toBe(false);
    });
  });

  describe("auto-pass senders", () => {
    it("passes a known domain on the bare domain", () => {
      const result = filterCalendarCandidate(
        make("noreply@eventbrite.com", "anything"),
      );
      expect(result).toEqual({ pass: true, reason: "known sender" });
    });

    it("passes a transactional subdomain of a known domain", () => {
      const result = filterCalendarCandidate(
        make("noreply@reminder.eventbrite.com", "Just added! BCIMS New Year's Retreat"),
      );
      expect(result).toEqual({ pass: true, reason: "known sender" });
    });

    it("does not pass a lookalike domain that merely ends with the same letters", () => {
      const result = filterCalendarCandidate(
        make("noreply@noteventbrite.com", "Updates to Our Privacy Policy"),
      );
      expect(result.pass).toBe(false);
    });
  });

  describe("real calendar emails pass on subject keywords", () => {
    it.each([
      ["no-reply@cortico.health", "Your Dr. Hassan Salame Appointment Reservation"],
      ["notifications@tribehome.com", "Reminder - Hard Surface Cleaning - Monday"],
      ["googleaistudio-noreply@google.com", "[Reminder] Secure your Gemini API access"],
    ])("passes %s — %s", (from, subject) => {
      expect(filterCalendarCandidate(make(from, subject)).pass).toBe(true);
    });
  });

  describe("footer/incidental noise is rejected", () => {
    it('does not pass on an "All rights reserved" footer', () => {
      const result = filterCalendarCandidate(
        make(
          "service@intl.paypal.com",
          "Your way to pay with PayPal is set",
          "Thanks for setting up PayPal.\n© 2026 PayPal. All rights reserved.",
        ),
      );
      expect(result.pass).toBe(false);
      expect(result.reason).toBe("no keyword match");
    });

    it("does not pass a privacy-policy email that merely mentions a concert", () => {
      const result = filterCalendarCandidate(
        make(
          "no-reply@legal.spotify.com",
          "Updates to Our Privacy Policy",
          "We may process data when you attend a concert or live event.",
        ),
      );
      expect(result.pass).toBe(false);
    });

    it("does not pass a terms-of-service email mentioning a support ticket", () => {
      const result = filterCalendarCandidate(
        make(
          "no-reply@mailgun.patreon.com",
          "Updates to Patreon's Terms and Privacy Policy",
          "Questions? Open a support ticket and we'll help.",
        ),
      );
      expect(result.pass).toBe(false);
    });
  });
});
