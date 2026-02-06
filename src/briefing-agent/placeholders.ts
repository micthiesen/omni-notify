import { resolveHistoryPlaceholders } from "./persistence.js";

export function resolveDatePlaceholder(prompt: string): string {
  return prompt.replace(/\{\{date\}\}/g, () => {
    const now = new Date();
    return now.toLocaleDateString("en-US", {
      weekday: "long",
      year: "numeric",
      month: "long",
      day: "numeric",
    });
  });
}

export function resolveTimePlaceholder(prompt: string): string {
  return prompt.replace(/\{\{time\}\}/g, () => {
    const now = new Date();
    return now.toLocaleTimeString("en-US", {
      hour: "numeric",
      minute: "2-digit",
      timeZoneName: "short",
    });
  });
}

export function resolveAllPlaceholders(prompt: string, briefingName: string): string {
  let resolved = prompt;
  resolved = resolveHistoryPlaceholders(resolved, briefingName);
  resolved = resolveDatePlaceholder(resolved);
  resolved = resolveTimePlaceholder(resolved);
  return resolved;
}
