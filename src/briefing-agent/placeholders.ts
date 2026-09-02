import { resolveHistoryPlaceholders } from "./persistence.js";
import { Clock, Effect } from "effect";

export function resolveDatePlaceholder(prompt: string, now = new Date()): string {
  return prompt.replace(/\{\{date\}\}/g, () => {
    return now.toLocaleDateString("en-US", {
      weekday: "long",
      year: "numeric",
      month: "long",
      day: "numeric",
    });
  });
}

export function resolveTimePlaceholder(prompt: string, now = new Date()): string {
  return prompt.replace(/\{\{time\}\}/g, () => {
    return now.toLocaleTimeString("en-US", {
      hour: "numeric",
      minute: "2-digit",
      timeZoneName: "short",
    });
  });
}

export function resolveAllPlaceholders(prompt: string, briefingName: string) {
  return Effect.gen(function* () {
    const now = new Date(yield* Clock.currentTimeMillis);
    let resolved = yield* resolveHistoryPlaceholders(prompt, briefingName);
    resolved = resolveDatePlaceholder(resolved, now);
    resolved = resolveTimePlaceholder(resolved, now);
    return resolved;
  });
}
