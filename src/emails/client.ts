import type { Transporter } from "nodemailer";
import nodemailer from "nodemailer";

import config from "../utils/config.js";

let transporter: Transporter | null = null;

export interface ComposeEmailConfiguration {
  host: string;
  port: number;
  user: string;
  pass: string;
  from: string;
  secure: boolean;
  requireTLS: boolean;
  source: "smtp" | "icloud";
}

export function resolveComposeEmailConfiguration(
  values: Pick<
    typeof config,
    | "SMTP_HOST"
    | "SMTP_PORT"
    | "SMTP_USER"
    | "SMTP_PASS"
    | "EMAIL_FROM"
    | "ICLOUD_USERNAME"
    | "ICLOUD_APP_PASSWORD"
  > = config,
): ComposeEmailConfiguration | undefined {
  if (values.SMTP_HOST && values.SMTP_USER && values.SMTP_PASS) {
    return {
      host: values.SMTP_HOST,
      port: values.SMTP_PORT,
      user: values.SMTP_USER,
      pass: values.SMTP_PASS,
      from: values.EMAIL_FROM || values.SMTP_USER,
      secure: values.SMTP_PORT === 465,
      requireTLS: values.SMTP_PORT !== 465,
      source: "smtp",
    };
  }
  if (values.SMTP_HOST || values.SMTP_USER || values.SMTP_PASS) return undefined;
  if (values.ICLOUD_USERNAME && values.ICLOUD_APP_PASSWORD) {
    return {
      host: "smtp.mail.me.com",
      port: 587,
      user: values.ICLOUD_USERNAME,
      pass: values.ICLOUD_APP_PASSWORD,
      from: values.EMAIL_FROM || values.ICLOUD_USERNAME,
      secure: false,
      requireTLS: true,
      source: "icloud",
    };
  }
  return undefined;
}

export function getComposeEmailConfiguration(): ComposeEmailConfiguration | undefined {
  return resolveComposeEmailConfiguration();
}

export function getTransporter(): Transporter | null {
  const settings = getComposeEmailConfiguration();
  if (!settings) return null;

  if (!transporter) {
    transporter = nodemailer.createTransport({
      host: settings.host,
      port: settings.port,
      secure: settings.secure,
      requireTLS: settings.requireTLS,
      auth: {
        user: settings.user,
        pass: settings.pass,
      },
    });
  }

  return transporter;
}
