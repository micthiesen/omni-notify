import { escapeXml } from "@micthiesen/mitools/xml";

export const QUOTE_START_RE = /^\s*(&gt;)+\s*/i;

// Chapter markers the cleaner emits as "## Short Title" lines. Podcast clients
// render the description as HTML, so surface them as bold headings rather than
// leaking the literal "## " markdown into the feed.
export const HEADING_RE = /^\s*##\s+(.+?)\s*$/;

/** Escape narration text for the feed, styling blockquotes and headings. */
export const prepareTextForRss = (text: string | undefined): string => {
  if (!text) return "";

  const lines = escapeXml(text).split("\n");
  const linesModified = lines.map((line) => {
    const heading = line.match(HEADING_RE);
    if (heading) {
      return `<b>${heading[1]}</b>`;
    }
    if (isBlockQuote(line)) {
      return `<i>${line.replace(QUOTE_START_RE, "")}</i>`;
    }
    return line;
  });

  return linesModified.join("<br>");
};

export const isBlockQuote = (line: string | undefined): boolean =>
  QUOTE_START_RE.test(line ?? "");
