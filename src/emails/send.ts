import type { LogItem } from "@micthiesen/mitools/logging";
import { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";

import { runPromise } from "../effect/interop.js";
import config from "../utils/config.js";
import { getTransporter } from "./client.js";
import { type EmailContent, renderLogEmail } from "./templates.js";

const logger = new Logger("Email");

interface SendEmailParams {
  to: string;
  from: string;
  subject: string;
  html: string;
  text: string;
}

export function sendEmailEffect(params: SendEmailParams): Effect.Effect<boolean> {
  return Effect.gen(function* () {
    const { to, from, subject, html, text } = params;
    const transporter = getTransporter();
    if (!transporter) {
      logger.debug("SMTP not configured, skipping email");
      return false;
    }
    return yield* Effect.tryPromise(() =>
      transporter.sendMail({ from, to, subject, html, text }),
    ).pipe(
      Effect.as(true),
      Effect.tap(() =>
        Effect.sync(() => logger.debug(`Email sent: "${subject}" to ${to}`)),
      ),
      Effect.catchAll((error) =>
        Effect.sync(() => {
          logger.error(`Failed to send email "${subject}" to ${to}`, error);
          return false;
        }),
      ),
    );
  });
}

export function sendEmail(params: SendEmailParams): Promise<boolean> {
  return runPromise(sendEmailEffect(params));
}

export function sendLogEmailEffect(
  subject: string,
  logs: LogItem[],
): Effect.Effect<boolean> {
  return Effect.gen(function* () {
    const { EMAIL_FROM, LOGS_EMAIL_TO } = config;

    if (!EMAIL_FROM || !LOGS_EMAIL_TO) {
      logger.debug("Log email not configured, skipping");
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

export function sendLogEmail(subject: string, logs: LogItem[]): Promise<boolean> {
  return runPromise(sendLogEmailEffect(subject, logs));
}
