import type { LogItem } from "@micthiesen/mitools/logging";

export interface EmailContent {
  html: string;
  text: string;
}

export function renderLogEmail(subject: string, logs: LogItem[]): EmailContent {
  const htmlLines = logs.map((log) => {
    const color = getLogColor(log.level);
    const args = log.args.length > 0 ? ` ${formatArgs(log.args)}` : "";
    return `<span style="color: ${color}">[${log.level.toUpperCase()}]</span> ${escapeHtml(log.message)}${escapeHtml(args)}`;
  });

  const textLines = logs.map((log) => {
    const args = log.args.length > 0 ? ` ${formatArgs(log.args)}` : "";
    return `[${log.level.toUpperCase()}] ${log.message}${args}`;
  });

  const html = `<!DOCTYPE html>
<html>
<head>
  <meta charset="utf-8">
  <title>${escapeHtml(subject)}</title>
</head>
<body style="font-family: system-ui, -apple-system, sans-serif; padding: 20px; max-width: 800px;">
  <h1 style="font-size: 18px; margin-bottom: 16px;">${escapeHtml(subject)}</h1>
  <pre style="background: #f5f5f5; padding: 16px; border-radius: 4px; overflow-x: auto; font-family: 'SF Mono', Monaco, Consolas, monospace; font-size: 13px; line-height: 1.4;">${htmlLines.join("\n")}</pre>
</body>
</html>`;

  const text = `${subject}\n${"=".repeat(subject.length)}\n\n${textLines.join("\n")}`;

  return { html, text };
}

function getLogColor(level: string): string {
  switch (level) {
    case "debug":
      return "#888";
    case "info":
      return "#333";
    case "warn":
      return "#b45309";
    case "error":
      return "#dc2626";
    default:
      return "#333";
  }
}

function formatArgs(args: unknown[]): string {
  return args.map((arg) => JSON.stringify(arg)).join(" ");
}

function escapeHtml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}
