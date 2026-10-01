import type { LogItem } from "@micthiesen/mitools/logging";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect, Schema } from "effect";
import type { SentMessageInfo, Transporter } from "nodemailer";

import config from "../utils/config.js";
import { getTransporter } from "./client.js";
import { type EmailContent, renderLogEmail } from "./templates.js";

const logger = Logger.named("Email");
const DeliveryResultSchema = Schema.Struct({
  accepted: Schema.Array(Schema.String),
  rejected: Schema.Array(Schema.String),
});

interface SendEmailParams {
  to: string;
  from: string;
  subject: string;
  html: string;
  text: string;
}

export interface ComposedEmailParams {
  to: string[];
  cc?: string[];
  bcc?: string[];
  from: string;
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
): Effect.Effect<boolean, never, Logger> {
  return Effect.gen(function* () {
    const { to, from, subject, html, text } = params;
    const transporter = getTransporter();
    if (!transporter) {
      yield* logger.debug("SMTP not configured, skipping email");
      return false;
    }
    return yield* Effect.tryPromise(() =>
      transporter.sendMail({ from, to, subject, html, text }),
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
      yield* logger.error("SMTP is not configured for composed email");
      return false;
    }
    return yield* Effect.tryPromise({
      try: () =>
        transport.sendMail({
          ...(params.raw
            ? {
                raw: params.raw,
                envelope: {
                  from: params.from,
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
          from: params.from,
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
    const { EMAIL_FROM, LOGS_EMAIL_TO } = config;

    if (!EMAIL_FROM || !LOGS_EMAIL_TO) {
      yield* logger.debug("Log email not configured, skipping");
      return false;
    }

    const { html, text }: EmailContent = renderLogEmail(subject, logs);
    return yield* sendEmailEffect({
      to: LOGS_EMAIL_TO,
      from: EMAIL_FROM,
      subject,
      html,
      text,
    });
  });
}
