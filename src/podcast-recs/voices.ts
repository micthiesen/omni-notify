import { readFileSync } from "node:fs";
import config from "../utils/config.js";

const SECTION_HEADER_RE = /^##\s+voices\b/i;
const HEADER_RE = /^##(?!#)/;
const LIST_ITEM_RE = /^\s*[-*]\s+(.+)$/;
const TRAILING_PAREN_RE = /\s*\([^()]*\)\s*$/;
const TRAILING_ANNOTATION_RE = /\s*[—:]\s*[^—:]*$/;

/**
 * Parses the "## Voices ..." section of the podcast taste profile markdown
 * into a flat list of person names — people whose podcast guest appearances
 * should be recommended anywhere they turn up.
 */
export function parseVoices(markdown: string): string[] {
  const lines = markdown.split("\n");
  const startIndex = lines.findIndex((line) => SECTION_HEADER_RE.test(line));
  if (startIndex === -1) return [];

  const sectionLines: string[] = [];
  for (let i = startIndex + 1; i < lines.length; i++) {
    const line = lines[i];
    if (line === undefined) continue;
    if (HEADER_RE.test(line)) break;
    sectionLines.push(line);
  }

  const seen = new Set<string>();
  const names: string[] = [];
  for (const line of sectionLines) {
    const match = line.match(LIST_ITEM_RE);
    if (!match) continue;
    const raw = match[1];
    if (!raw) continue;
    const cleaned = cleanVoiceItem(raw);
    if (!cleaned) continue;

    const key = cleaned.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    names.push(cleaned);
  }

  return names;
}

function cleanVoiceItem(raw: string): string | undefined {
  let cleaned = raw.trim();
  cleaned = cleaned.replace(TRAILING_PAREN_RE, "");
  cleaned = cleaned.replace(TRAILING_ANNOTATION_RE, "");
  cleaned = cleaned.trim();

  if (cleaned.length < 2) return undefined;
  if (cleaned.startsWith("<")) return undefined;

  return cleaned;
}

/** Reads and parses the voices section from the configured taste profile. */
export function loadVoices(): string[] {
  const path = config.PODCAST_TASTE_PATH;
  if (!path) return [];
  try {
    return parseVoices(readFileSync(path, "utf8"));
  } catch {
    return [];
  }
}
