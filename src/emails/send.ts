import type { LogItem } from "@micthiesen/mitools/logging";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect, Schema } from "effect";
import { simpleParser } from "mailparser";
import type { SentMessageInfo, Transporter } from "nodemailer";

import config from "../utils/config.js";
import { OUTGOING_EMAIL_FROM } from "./identity.js";
import { getTransporter } from "./client.js";
import { type EmailContent, renderLogEmail } from "./templates.js";

const logger = Logger.named("Email");
const DeliveryResultSchema = Schema.Struct({
  accepted: Schema.Array(Schema.String),
  rejected: Schema.Array(Schema.String),
});

interface SendEmailParams {
  to: string;
  subject: string;
  html: string;
  text: string;
}

export interface ComposedEmailParams {
  to: string[];
  cc?: string[];
  bcc?: string[];
  subject: string;
  text: string;
  inReplyTo?: string;
  references?: string[];
  messageId?: string;
  /** Prebuilt Bcc-free SMTP MIME, persisted before submitting. */
  raw?: Buffer;
}

export function sendEmailEffect(
  params: SendEmailParams,
  transport: Pick<Transporter<SentMessageInfo>, "sendMail"> | null = getTransporter(),
): Effect.Effect<boolean, never, Logger> {
  return Effect.gen(function* () {
    const { to, subject, html, text } = params;
    if (!transport) {
      yield* logger.error(`SMTP is not configured to send as ${OUTGOING_EMAIL_FROM}`);
      return false;
    }
    return yield* Effect.tryPromise(() =>
      transport.sendMail({ from: OUTGOING_EMAIL_FROM, to, subject, html, text }),
    ).pipe(
      Effect.as(true),
      Effect.tap(() => logger.debug(`Email sent: "${subject}" to ${to}`)),
      Effect.catch((error) =>
        logger
          .error(`Failed to send email "${subject}" to ${to}`, error)
          .pipe(Effect.as(false)),
      ),
    );
  });
}

/** Send user-composed mail; the caller must reserve its idempotency key first. */
export function sendComposedEmailEffect(
  params: ComposedEmailParams,
  transport: Pick<Transporter<SentMessageInfo>, "sendMail"> | null = getTransporter(),
): Effect.Effect<boolean, never, Logger> {
  return Effect.gen(function* () {
    if (!transport) {
      yield* logger.error(`SMTP is not configured to send as ${OUTGOING_EMAIL_FROM}`);
      return false;
    }
    const raw = params.raw;
    if (raw) {
      const validSender = yield* Effect.tryPromise(() => simpleParser(raw)).pipe(
        Effect.map(
          (mail) =>
            mail.from?.value.length === 1 &&
            mail.from.value[0].address === OUTGOING_EMAIL_FROM &&
            !mail.headers.has("sender") &&
            !mail.headers.has("resent-from"),
        ),
        Effect.catch(() => Effect.succeed(false)),
      );
      if (!validSender) {
        yield* logger.error(
          `Persisted email must be from ${OUTGOING_EMAIL_FROM}; SMTP submission refused`,
        );
        return false;
      }
    }
    return yield* Effect.tryPromise({
      try: () =>
        transport.sendMail({
          ...(params.raw
            ? {
                raw: params.raw,
                envelope: {
                  from: OUTGOING_EMAIL_FROM,
                  to: [
                    ...new Set([
                      ...params.to,
                      ...(params.cc ?? []),
                      ...(params.bcc ?? []),
                    ]),
                  ],
                },
              }
            : {}),
          from: OUTGOING_EMAIL_FROM,
          to: params.to,
          cc: params.cc,
          bcc: params.bcc,
          subject: params.subject,
          text: params.text,
          inReplyTo: params.inReplyTo,
          references: params.references,
          messageId: params.messageId,
        }),
      catch: (cause) => cause,
    }).pipe(
      Effect.flatMap((result) =>
        Schema.decodeUnknownEffect(DeliveryResultSchema)(result),
      ),
      Effect.map((result) => {
        const expected = new Set([
          ...params.to,
          ...(params.cc ?? []),
          ...(params.bcc ?? []),
        ]);
        const accepted = new Set(result.accepted);
        return (
          result.rejected.length === 0 &&
          [...expected].every((address) => accepted.has(address))
        );
      }),
      Effect.tap((sent) =>
        sent
          ? Effect.void
          : logger.warn("SMTP rejected one or more composed email recipients"),
      ),
      Effect.catch((error) =>
        logger.error("Failed to send composed email", error).pipe(Effect.as(false)),
      ),
    );
  });
}

export function sendLogEmailEffect(
  subject: string,
  logs: LogItem[],
): Effect.Effect<boolean, never, Logger> {
  return Effect.gen(function* () {
    const { LOGS_EMAIL_TO } = config;

    if (!LOGS_EMAIL_TO) {
      yield* logger.debug("Log email not configured, skipping");
      return false;
    }

    const { html, text }: EmailContent = renderLogEmail(subject, logs);
    return yield* sendEmailEffect({
      to: LOGS_EMAIL_TO,
      subject,
      html,
      text,
    });
  });
}
