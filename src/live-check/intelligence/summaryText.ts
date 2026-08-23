const SUMMARY_MAX_CHARS = 220;
const TOPIC_MAX_CHARS = 60;

const TOPIC_STOP_WORDS = new Set([
  "a",
  "about",
  "and",
  "as",
  "at",
  "by",
  "for",
  "from",
  "in",
  "into",
  "of",
  "on",
  "over",
  "the",
  "to",
  "versus",
  "with",
]);

function normalizeSpacing(value: string): string {
  return value
    .normalize("NFKC")
    .replace(/[\p{Cc}\p{Cf}]/gu, "")
    .replace(/\s+/g, " ")
    .trim();
}

function stripConstrainedOutputArtifact(value: string): string {
  return value
    .replace(/[|#]{2,}$/u, "")
    .replace(/\s*\S*[\p{Script=Han}\p{Script=Thai}\p{Script=Bengali}]\S*$/u, "")
    .trim();
}

function truncateAtWord(value: string, maxChars: number): string {
  if (value.length <= maxChars) return value;
  const candidate = value.slice(0, maxChars - 1).trimEnd();
  const boundary = candidate.lastIndexOf(" ");
  const cut =
    boundary >= Math.floor(maxChars * 0.6) ? candidate.slice(0, boundary) : candidate;
  return `${cut.replace(/[,:;\-–—]+$/u, "").trimEnd()}…`;
}

/**
 * Keeps the model comfortably below its schema ceiling and turns any
 * constrained-decoding tail into a clean sentence or an explicit ellipsis.
 */
export function cleanLivestreamSummary(value: string): string {
  let cleaned = stripConstrainedOutputArtifact(normalizeSpacing(value));
  if (!cleaned) return "The current discussion could not be summarized cleanly.";
  cleaned = truncateAtWord(cleaned, SUMMARY_MAX_CHARS);
  if (/[.!?…]["'”’)]?$/u.test(cleaned)) return cleaned;

  const sentenceEnds = [...cleaned.matchAll(/[.!?]["'”’)]?(?=\s|$)/gu)];
  const lastComplete = sentenceEnds.at(-1);
  if (lastComplete?.index !== undefined) {
    const end = lastComplete.index + lastComplete[0].length;
    if (end >= Math.floor(cleaned.length * 0.45)) return cleaned.slice(0, end);
  }
  return `${cleaned.replace(/[,:;\-–—]+$/u, "").trimEnd()}…`;
}

export function cleanLivestreamTopic(value: string): string {
  const cleaned = stripConstrainedOutputArtifact(normalizeSpacing(value)).replace(
    /[,:;\-–—]+$/u,
    "",
  );
  return cleaned ? truncateAtWord(cleaned, TOPIC_MAX_CHARS) : "Current discussion";
}

function topicTokens(value: string): Set<string> {
  const normalized = normalizeSpacing(value)
    .toLowerCase()
    .replace(/[^a-z0-9 ]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  return new Set(
    normalized
      .split(" ")
      .map((token) => token.replace(/(?:ing|ed|es|s)$/u, ""))
      .filter((token) => token.length >= 3 && !TOPIC_STOP_WORDS.has(token)),
  );
}

/** Broader than exact labels, while still requiring shared subject words. */
export function areSameLivestreamTopic(a: string, b: string): boolean {
  const leftText = normalizeSpacing(a).toLowerCase();
  const rightText = normalizeSpacing(b).toLowerCase();
  if (leftText === rightText) return true;
  if (
    Math.min(leftText.length, rightText.length) >= 12 &&
    (leftText.includes(rightText) || rightText.includes(leftText))
  ) {
    return true;
  }

  const left = topicTokens(a);
  const right = topicTokens(b);
  if (left.size === 0 || right.size === 0) return false;
  const shared = [...left].filter((token) => right.has(token)).length;
  const containment = shared / Math.min(left.size, right.size);
  const union = new Set([...left, ...right]).size;
  return shared >= 2 && (containment >= 0.5 || shared / union >= 0.4);
}
