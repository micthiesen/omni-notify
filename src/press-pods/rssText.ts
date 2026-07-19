import { escapeXml } from "@micthiesen/mitools/xml";

export const QUOTE_START_RE = /^\s*(&gt;)+\s*/i;

/** Escape narration text for the feed, italicizing blockquote lines. */
export const prepareTextForRss = (text: string | undefined): string => {
  if (!text) return "";

  const lines = escapeXml(text).split("\n");
  const linesModified = lines.map((line) => {
    if (isBlockQuote(line)) {
      return `<i>${line.replace(QUOTE_START_RE, "")}</i>`;
    }
    return line;
  });

  return linesModified.join("<br>");
};

export const isBlockQuote = (line: string | undefined): boolean =>
  QUOTE_START_RE.test(line ?? "");
