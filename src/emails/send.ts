import type { LogItem } from "@micthiesen/mitools/logging";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";

import config from "../utils/config.js";
import { getTransporter } from "./client.js";
import { type EmailContent, renderLogEmail } from "./templates.js";

const logger = Logger.named("Email");

interface SendEmailParams {
  to: string;
  from: string;
  subject: string;
  html: string;
  text: string;
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
