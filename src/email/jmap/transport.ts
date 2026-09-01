import type { Logger } from "@micthiesen/mitools/logging";
import { Data, Effect, Exit, Schema, Scope } from "effect";
import { getLastDispatchedAtEffect } from "../persistence.js";
import type {
  DownloadedAttachment,
  EmailAttachment,
  EmailPoll,
  EmailTransport,
  FetchedEmail,
} from "../types.js";
import { createJmapClientEffect, type JmapContext } from "./client.js";
import {
  fetchEmailByIdEffect,
  fetchEmailsReceivedSinceEffect,
  fetchNewEmailsEffect,
} from "./emailFetcher.js";
import { createEventSourceEffect } from "./eventSource.js";
import { getEmailStateEffect, saveEmailState } from "./persistence.js";

/** Overlap window when recovering from a JMAP state reset: re-query emails
 * received up to this long before the last dispatch (pipelines dedup). */
const RECOVERY_OVERLAP_MS = 60 * 60_000;
export const MAX_JMAP_ATTACHMENT_BYTES = 5 * 1024 * 1024;
const EmailStateResponseSchema = Schema.Struct({
  state: Schema.optional(Schema.String),
});

export class JmapTransportError extends Data.TaggedError("JmapTransportError")<{
  readonly operation: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `${this.operation} failed: ${detail}`;
  }
}

/**
 * Consume the response body incrementally so an incorrect or missing JMAP
 * attachment size cannot make us buffer an unbounded response. The scoped
 * reader finalizer cancels an in-flight read when the Effect is interrupted.
 */
export function readJmapAttachmentBodyEffect(
  response: Response,
): Effect.Effect<Buffer, JmapTransportError> {
  if (!response.body) return Effect.succeed(Buffer.alloc(0));

  return Effect.acquireUseRelease(
    Effect.sync(() => response.body!.getReader()),
    (reader) =>
      Effect.gen(function* () {
        const chunks: Uint8Array[] = [];
        let byteLength = 0;

        while (true) {
          const next = yield* Effect.tryPromise({
            try: () => reader.read(),
            catch: (cause) =>
              new JmapTransportError({ operation: "read attachment", cause }),
          });
          if (next.done) break;

          byteLength += next.value.byteLength;
          if (byteLength > MAX_JMAP_ATTACHMENT_BYTES) {
            return yield* new JmapTransportError({
              operation: "read attachment",
              cause: new Error(
                `attachment exceeds ${MAX_JMAP_ATTACHMENT_BYTES} byte limit`,
              ),
            });
          }
          chunks.push(next.value);
        }

        return Buffer.concat(chunks, byteLength);
      }),
    (reader) =>
      Effect.promise(() => reader.cancel()).pipe(
        Effect.ignore,
        Effect.ensuring(Effect.sync(() => reader.releaseLock())),
      ),
  );
}

export function downloadJmapAttachmentEffect(
  ctx: JmapContext,
  logger: Logger,
  attachment: EmailAttachment,
): Effect.Effect<DownloadedAttachment | undefined, never> {
  return Effect.acquireUseRelease(
    Effect.sync(() => new AbortController()),
    (controller) =>
      Effect.tryPromise({
        try: () =>
          ctx.jam.downloadBlob(
            {
              accountId: ctx.accountId,
              blobId: attachment.blobId,
              mimeType: attachment.type,
              fileName: attachment.name,
            },
            { signal: controller.signal },
          ),
        catch: (cause) =>
          new JmapTransportError({ operation: "download attachment", cause }),
      }).pipe(
        Effect.flatMap((response) =>
          Effect.gen(function* () {
            if (!response.ok) {
              logger.warn(
                `Failed to download "${attachment.name}": ${response.status} ${response.statusText}`,
              );
              return;
            }

            const data = yield* readJmapAttachmentBodyEffect(response);
            return {
              name: attachment.name,
              mimeType: attachment.type,
              data,
            };
          }),
        ),
      ),
    (controller) => Effect.sync(() => controller.abort()),
  ).pipe(
    Effect.catch((error) =>
      Effect.sync(() => {
        logger.warn(`Error downloading "${attachment.name}"`, error.message);
        return undefined;
      }),
    ),
  );
}

/**
 * Fastmail transport: JMAP Email/changes deltas + SSE EventSource push.
 * Stays alive until the iCloud MX cutover completes; selected via
 * EMAIL_TRANSPORT=fastmail (the default while FASTMAIL_API_TOKEN is set).
 */
export class JmapTransport implements EmailTransport<JmapTransportError> {
  public readonly name = "JMAP";

  private ctx: JmapContext;
  private logger: Logger;
  private eventSourceScope: Scope.Closeable | undefined;

  private constructor(ctx: JmapContext, logger: Logger) {
    this.ctx = ctx;
    this.logger = logger;
  }

  static createEffect(
    bearerToken: string,
    logger: Logger,
  ): Effect.Effect<JmapTransport, JmapTransportError> {
    return createJmapClientEffect(bearerToken, logger).pipe(
      Effect.mapError(
        (cause) => new JmapTransportError({ operation: "create client", cause }),
      ),
      Effect.map((ctx) => new JmapTransport(ctx, logger)),
    );
  }

  startEffect(onMailEvent: () => void): Effect.Effect<void, JmapTransportError> {
    return Effect.uninterruptibleMask((restore) =>
      Effect.gen({ self: this }, function* () {
        const previousScope = this.eventSourceScope;
        this.eventSourceScope = undefined;
        if (previousScope) {
          yield* Scope.close(previousScope, Exit.succeed(undefined));
        }
        const scope = yield* Scope.make();
        yield* restore(
          createEventSourceEffect(this.ctx, onMailEvent, this.logger).pipe(
            Effect.provideService(Scope.Scope, scope),
            Effect.mapError(
              (cause) =>
                new JmapTransportError({ operation: "start event source", cause }),
            ),
          ),
        ).pipe(
          Effect.onExit((exit) =>
            Exit.isSuccess(exit) ? Effect.void : Scope.close(scope, exit),
          ),
        );
        // Back in the uninterruptible region: ownership cannot be interrupted
        // between successful acquisition and retaining the Scope.
        this.eventSourceScope = scope;
      }),
    );
  }

  readonly stopEffect: Effect.Effect<void, never> = Effect.suspend(() => {
    const scope = this.eventSourceScope;
    this.eventSourceScope = undefined;
    return scope ? Scope.close(scope, Exit.succeed(undefined)) : Effect.void;
  });

  readonly pollNewEmailsEffect: Effect.Effect<EmailPoll, JmapTransportError> =
    Effect.gen({ self: this }, function* () {
      const sinceState = yield* getEmailStateEffect.pipe(
        Effect.mapError(
          (cause) => new JmapTransportError({ operation: "read state", cause }),
        ),
      );

      if (!sinceState) {
        this.logger.info("First run: fetching current JMAP state (skipping history)");
        const state = yield* this.fetchCurrentEmailStateEffect;
        return {
          emails: [],
          commit: () => {
            if (state) {
              saveEmailState(state);
              this.logger.info(`Saved initial JMAP state: ${state}`);
            }
          },
        };
      }

      return yield* fetchNewEmailsEffect(this.ctx, sinceState, this.logger).pipe(
        Effect.mapError(
          (cause) => new JmapTransportError({ operation: "fetch changes", cause }),
        ),
        Effect.map(({ emails, newState }) => ({
          emails,
          commit: () => saveEmailState(newState),
        })),
        Effect.catch((error) =>
          error.message.includes("cannotCalculateChanges")
            ? this.recoverFromStateResetEffect
            : Effect.fail(error),
        ),
      );
    });

  /**
   * The server can no longer diff from our saved state. Instead of silently
   * resetting (which drops everything received in the gap), run a bounded
   * Email/query for mail received after the last dispatch (minus an overlap)
   * and resume from the fresh state.
   */
  private readonly recoverFromStateResetEffect: Effect.Effect<
    EmailPoll,
    JmapTransportError
  > = Effect.gen({ self: this }, function* () {
    const lastDispatchedAt = yield* getLastDispatchedAtEffect.pipe(
      Effect.mapError(
        (cause) =>
          new JmapTransportError({ operation: "read dispatch watermark", cause }),
      ),
    );

    if (lastDispatchedAt === undefined) {
      this.logger.warn(
        "cannotCalculateChanges: JMAP state was reset with no last-dispatch " +
          "timestamp to recover from; resetting state only",
      );
      const state = yield* this.fetchCurrentEmailStateEffect;
      return {
        emails: [],
        commit: () => {
          if (state) saveEmailState(state);
        },
      };
    }

    const sinceMs = lastDispatchedAt - RECOVERY_OVERLAP_MS;
    const { emails, state } = yield* fetchEmailsReceivedSinceEffect(
      this.ctx,
      sinceMs,
      this.logger,
    ).pipe(
      Effect.mapError(
        (cause) => new JmapTransportError({ operation: "recover state", cause }),
      ),
    );
    this.logger.warn(
      `cannotCalculateChanges: JMAP state was reset; recovered ${emails.length} ` +
        `email(s) received since ${new Date(sinceMs).toISOString()}`,
    );
    return { emails, commit: () => saveEmailState(state) };
  });

  fetchEmailByIdEffect(
    emailId: string,
  ): Effect.Effect<FetchedEmail | undefined, JmapTransportError> {
    return fetchEmailByIdEffect(this.ctx, emailId, this.logger).pipe(
      Effect.mapError(
        (cause) => new JmapTransportError({ operation: "fetch email", cause }),
      ),
    );
  }

  downloadAttachmentEffect(
    attachment: EmailAttachment,
  ): Effect.Effect<DownloadedAttachment | undefined, never> {
    return downloadJmapAttachmentEffect(this.ctx, this.logger, attachment);
  }

  private readonly fetchCurrentEmailStateEffect = Effect.tryPromise({
    try: () =>
      this.ctx.jam.request(["Email/get", { accountId: this.ctx.accountId, ids: [] }]),
    catch: (cause) => new JmapTransportError({ operation: "fetch state", cause }),
  }).pipe(
    Effect.flatMap(([result]) =>
      Schema.decodeUnknownEffect(EmailStateResponseSchema)(result).pipe(
        Effect.map((decoded) => decoded.state),
        Effect.mapError(
          (cause) => new JmapTransportError({ operation: "decode state", cause }),
        ),
      ),
    ),
  );
}
