import { convert } from "html-to-text";

export function htmlToText(html: string): string {
  return convert(html, {
    wordwrap: false,
    decodeEntities: true,
    selectors: [
      { selector: "a", options: { ignoreHref: true } },
      { selector: "img", format: "skip" },
      { selector: "h1", options: { uppercase: false } },
      { selector: "h2", options: { uppercase: false } },
      { selector: "h3", options: { uppercase: false } },
      { selector: "hr", format: "skip" },
      { selector: "table", options: { colSpacing: 2 } },
    ],
  });
}
