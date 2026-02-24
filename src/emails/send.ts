import type { LogItem } from "@micthiesen/mitools/logging";
import { Logger } from "@micthiesen/mitools/logging";

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

export async function sendEmail(params: SendEmailParams): Promise<boolean> {
  const { to, from, subject, html, text } = params;

  const transporter = getTransporter();
  if (!transporter) {
    logger.debug("SMTP not configured, skipping email");
    return false;
  }

  try {
    await transporter.sendMail({ from, to, subject, html, text });
    logger.debug(`Email sent: "${subject}" to ${to}`);
    return true;
  } catch (error) {
    logger.error(`Failed to send email "${subject}" to ${to}`, error);
    return false;
  }
}

export async function sendLogEmail(subject: string, logs: LogItem[]): Promise<boolean> {
  const { EMAIL_FROM, LOGS_EMAIL_TO } = config;

  if (!EMAIL_FROM || !LOGS_EMAIL_TO) {
    logger.debug("Log email not configured, skipping");
    return false;
  }

  const { html, text }: EmailContent = renderLogEmail(subject, logs);

  return sendEmail({
    to: LOGS_EMAIL_TO,
    from: EMAIL_FROM,
    subject,
    html,
    text,
  });
}
