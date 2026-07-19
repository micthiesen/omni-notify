import { format } from "date-fns";
import { convert } from "html-to-text";
import { removeExtraEmptyLines } from "./lineFiltering.js";

export function cleanText(dirtyText: string): string {
  if (dirtyText.length < 200) {
    throw new Error(`Article is too short: ${dirtyText}`);
  }

  const text = convert(dirtyText, {
    wordwrap: false,
    decodeEntities: true,
    encodeCharacters: {},
    selectors: [
      { selector: "a", options: { ignoreHref: true } },
      { selector: "img", format: "skip" },
      { selector: "h1", options: { uppercase: false } },
      { selector: "h2", options: { uppercase: false } },
      { selector: "h3", options: { uppercase: false } },
      { selector: "hr", format: "skip" },
      { selector: "table", format: "skip" },
    ],
  });
  const lines = text.split("\n").map(standardizePrefix);
  return removeExtraEmptyLines(lines).join("\n");
}

export function buildFinalText(result: {
  title?: string | null;
  domain?: string | null;
  author: string;
  coauthors?: string[] | null;
  datePublished?: Date | null;
  text: string;
}): string {
  const date = formatDateSafe(result.datePublished);
  const finalText =
    `${result.title ? `${result.title}. ` : ""}` +
    `By ${result.author}${result.coauthors?.length ? ` and ${result.coauthors.join(", ")}` : ""}. ` +
    `${
      date ? `Published ${date}${result.domain ? ` on ${result.domain}` : ""}. ` : ""
    }` +
    `\n\n${result.text}`;
  return finalText;
}

function formatDateSafe(date: Date | null | undefined): string | null {
  try {
    return date ? format(date, "MMMM d, yyyy") : null;
  } catch {
    return null;
  }
}

function standardizePrefix(line: string): string {
  // For some reason some quoted lines start with "> > " instead of "> "
  return line.replace(/^\s*[>][>\s]*/i, "> ");
}
