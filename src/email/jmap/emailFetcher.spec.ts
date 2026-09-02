import { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
import type { JamClient } from "jmap-jam";
import { describe, expect, it } from "vitest";
import type { JmapContext } from "./client.js";
import {
  fetchEmailByIdEffect,
  fetchEmailsReceivedSinceEffect,
  fetchNewEmailsEffect,
  JmapFetchError,
  RECOVERY_QUERY_LIMIT,
} from "./emailFetcher.js";

const logger = new Logger("JmapEmailFetcherTest");

function contextWithResponse(response: unknown): JmapContext {
  return {
    accountId: "account-1",
    jam: {
      request: () => Promise.resolve([response]),
    } as unknown as JamClient,
  };
}

describe("JMAP Email/get decoding", () => {
  it("drains every recovery query page before returning the captured state", async () => {
    const rawEmail = (id: string) => ({
      id,
      subject: id,
      mailboxIds: { inbox: true },
      receivedAt: "2026-09-01T12:00:00Z",
    });
    const firstIds = Array.from(
      { length: RECOVERY_QUERY_LIMIT },
      (_, index) => `email-${index}`,
    );
    const secondIds = ["email-200"];
    let requestCount = 0;
    let page = 0;
    const ctx: JmapContext = {
      accountId: "account-1",
      jam: {
        request: () => {
          requestCount++;
          return Promise.resolve([
            requestCount === 1
              ? { list: [{ id: "inbox", role: "inbox" }] }
              : { state: "recovery-state", list: [] },
          ]);
        },
        requestMany: () => {
          const ids = page++ === 0 ? firstIds : secondIds;
          return Promise.resolve([
            {
              query: { ids },
              emails: { state: "later-state", list: ids.map(rawEmail) },
            },
          ]);
        },
      } as unknown as JamClient,
    };

    const result = await Effect.runPromise(
      fetchEmailsReceivedSinceEffect(ctx, 0, logger),
    );

    expect(result.emails).toHaveLength(RECOVERY_QUERY_LIMIT + 1);
    expect(result.state).toBe("recovery-state");
    expect(page).toBe(2);
  });

  it("decodes every consumed nested email field before mapping", async () => {
    const ctx = contextWithResponse({
      state: "state-2",
      list: [
        {
          id: "email-1",
          subject: "Delivery update",
          from: [{ name: "Carrier", email: "carrier@example.com" }],
          textBody: [{ partId: "plain", type: "text/plain" }],
          htmlBody: [{ partId: "html", type: "text/html" }],
          bodyValues: {
            plain: { value: "Plain body" },
            html: { value: "<p>HTML body</p>" },
          },
          receivedAt: "2026-09-01T12:00:00Z",
          attachments: [
            {
              blobId: "blob-1",
              name: "label.pdf",
              type: "application/pdf",
              size: 123,
            },
          ],
          mailboxIds: { inbox: true },
        },
      ],
    });

    const email = await Effect.runPromise(fetchEmailByIdEffect(ctx, "email-1", logger));

    expect(email).toMatchObject({
      id: "email-1",
      subject: "Delivery update",
      from: "carrier@example.com",
      textBody: "HTML body",
      receivedAt: "2026-09-01T12:00:00Z",
      attachments: [
        {
          blobId: "blob-1",
          name: "label.pdf",
          type: "application/pdf",
          size: 123,
        },
      ],
    });
  });

  it("returns a typed fetch error when list is not an array", async () => {
    const ctx = contextWithResponse({ state: "state-2", list: {} });

    const error = await Effect.runPromise(
      fetchEmailByIdEffect(ctx, "email-1", logger).pipe(Effect.flip),
    );

    expect(error).toBeInstanceOf(JmapFetchError);
    expect(error.operation).toBe("decode Email/get");
  });

  it("returns a typed fetch error for a malformed nested list element", async () => {
    const ctx = contextWithResponse({
      state: "state-2",
      list: [
        {
          id: "email-1",
          from: [{ email: 42 }],
        },
      ],
    });

    const error = await Effect.runPromise(
      fetchEmailByIdEffect(ctx, "email-1", logger).pipe(Effect.flip),
    );

    expect(error).toBeInstanceOf(JmapFetchError);
    expect(error.operation).toBe("decode Email/get");
  });

  it("rejects malformed email elements returned by Email/changes", async () => {
    const ctx: JmapContext = {
      accountId: "account-1",
      jam: {
        request: () => Promise.resolve([{ list: [{ id: "inbox", role: "inbox" }] }]),
        requestMany: () =>
          Promise.resolve([
            {
              changes: { newState: "state-2", hasMoreChanges: false },
              emails: {
                state: "state-2",
                list: [
                  {
                    id: "email-1",
                    bodyValues: { plain: { value: 42 } },
                  },
                ],
              },
            },
          ]),
      } as unknown as JamClient,
    };

    const error = await Effect.runPromise(
      fetchNewEmailsEffect(ctx, "state-1", logger).pipe(Effect.flip),
    );

    expect(error).toBeInstanceOf(JmapFetchError);
    expect(error.operation).toBe("decode changes");
  });
});
