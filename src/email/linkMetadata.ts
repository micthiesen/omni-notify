import { Schema } from "effect";
import { parseHTML } from "linkedom";
import type { ParsedMail } from "mailparser";

export const EmailLinkMetadataSchema = Schema.Struct({
  links: Schema.Array(
    Schema.Struct({
      url: Schema.String,
      label: Schema.String,
      source: Schema.Literals(["html", "text"]),
    }),
  ),
  linksTruncated: Schema.Boolean,
  listUnsubscribe: Schema.Struct({
    urls: Schema.Array(Schema.String),
    post: Schema.NullOr(Schema.Literal("List-Unsubscribe=One-Click")),
    present: Schema.Boolean,
    truncated: Schema.Boolean,
  }),
});

export type EmailLinkMetadata = typeof EmailLinkMetadataSchema.Type;
const SCAN_LIMIT = 1024 * 1024;
const URL_LIMIT = 4096;
const HEADER_LIMIT = 16 * 1024;
const HEADER_COUNT_LIMIT = 1000;
const preferredLabel =
  /unsubscribe|opt[ -]?out|preferences|manage (?:email|subscription)/i;

/** Preserve usable URL bytes. Parsing validates only; it never follows a URL. */
function safeUrl(raw: string): string | undefined {
  if (raw.length > URL_LIMIT || /[\u0000-\u0020\u007f\\]/.test(raw)) return undefined;
  if (/%(?:0[0-9a-f]|1[0-9a-f]|7f)/i.test(raw)) return undefined;
  if (!/^(?:https?:\/\/|mailto:)/i.test(raw)) return undefined;
  try {
    const url = new URL(raw);
    if (url.username || url.password) return undefined;
    if (url.protocol === "mailto:") return url.pathname ? raw : undefined;
    return url.hostname ? raw : undefined;
  } catch {
    return undefined;
  }
}

/** Inert, bounded metadata extraction. No raw headers or remote content escape. */
export function extractEmailLinkMetadata(parsed: ParsedMail): EmailLinkMetadata {
  const links: Array<EmailLinkMetadata["links"][number]> = [];
  const seen = new Map<string, number>();
  let linksTruncated = false;
  const addLink = (raw: string, label: string, source: "html" | "text") => {
    if (raw.length > URL_LIMIT) linksTruncated = true;
    const url = safeUrl(raw);
    if (!url) return;
    const cleanLabel = label
      .replace(/[\u0000-\u001f\u007f]/g, " ")
      .replace(/\s+/g, " ")
      .trim();
    if (cleanLabel.length > 200) linksTruncated = true;
    const existingIndex = seen.get(url);
    if (existingIndex !== undefined) {
      const existing = links[existingIndex];
      if (
        existing &&
        preferredLabel.test(cleanLabel) &&
        !preferredLabel.test(existing.label)
      ) {
        links[existingIndex] = { ...existing, label: cleanLabel.slice(0, 200) };
      }
      return;
    }
    seen.set(url, links.length);
    links.push({ url, label: cleanLabel.slice(0, 200), source });
  };
  if (typeof parsed.html === "string") {
    let html = parsed.html.slice(0, SCAN_LIMIT);
    if (parsed.html.length > SCAN_LIMIT) {
      linksTruncated = true;
      // Do not allow HTML parser recovery to invent a partial href at the bound.
      const lastOpen = html.lastIndexOf("<");
      if (lastOpen > html.lastIndexOf(">")) html = html.slice(0, lastOpen);
    }
    const { document } = parseHTML(html);
    for (const anchor of document.querySelectorAll("a[href]")) {
      addLink(anchor.getAttribute("href") ?? "", anchor.textContent ?? "", "html");
    }
  }
  // Mailparser synthesizes text from HTML, including image URLs and bracketed
  // hrefs. Treat text as a source only for messages without an HTML part.
  const plainText = typeof parsed.html === "string" ? "" : (parsed.text ?? "");
  const text = plainText.slice(0, SCAN_LIMIT);
  const textTruncated = plainText.length > SCAN_LIMIT;
  linksTruncated ||= textTruncated;
  for (const match of text.matchAll(/(?:https?:\/\/|mailto:)[^\s<>"']+/gi)) {
    if (textTruncated && match.index + match[0].length === text.length) continue;
    addLink(match[0], "", "text");
  }
  // Stable partition ensures footer subscription links survive large newsletters.
  links.sort(
    (a, b) =>
      Number(preferredLabel.test(b.label)) - Number(preferredLabel.test(a.label)),
  );
  linksTruncated ||= links.length > 50;

  const urls: string[] = [];
  let present = false;
  let truncated = parsed.headerLines.length > HEADER_COUNT_LIMIT;
  let scanned = 0;
  const postValues: string[] = [];
  for (const header of parsed.headerLines.slice(0, HEADER_COUNT_LIMIT)) {
    // Relevant names are short; avoid scanning attacker-sized unrelated keys.
    if (header.key.length > 32) continue;
    const key = header.key.toLowerCase();
    if (key !== "list-unsubscribe" && key !== "list-unsubscribe-post") continue;
    if (key === "list-unsubscribe") present = true;
    scanned += header.line.length;
    if (scanned > HEADER_LIMIT) {
      truncated = true;
      continue;
    }
    const value = header.line
      .replace(/\r?\n[ \t]+/g, " ")
      .replace(/^[^:]*:/, "")
      .trim();
    if (key === "list-unsubscribe-post") {
      postValues.push(value);
      continue;
    }
    for (const match of value.matchAll(/<([^<>]*)>/g)) {
      const raw = match[1].trim();
      if (raw.length > URL_LIMIT) truncated = true;
      const url = safeUrl(raw);
      if (!url || urls.includes(url)) continue;
      if (urls.length === 10) truncated = true;
      else urls.push(url);
    }
  }
  return {
    links: links.slice(0, 50),
    linksTruncated,
    listUnsubscribe: {
      urls,
      post:
        !truncated &&
        postValues.length === 1 &&
        postValues[0] === "List-Unsubscribe=One-Click"
          ? "List-Unsubscribe=One-Click"
          : null,
      present,
      truncated,
    },
  };
}
