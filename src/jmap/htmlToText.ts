import { convert } from "html-to-text";

// Tracking numbers and booking references frequently exist ONLY inside link
// URLs ("Track your package" buttons), which htmlToText strips. This pulls
// hrefs that look shipment/booking-shaped so extraction prompts can see them.
const LINK_PATTERN = /href\s*=\s*["']([^"']+)["']/gi;
const INTERESTING_LINK = /track|shipment|deliver|parcel|order|booking|reservation/i;
const MAX_LINKS = 20;
const MAX_LINK_LENGTH = 500;

export function extractInterestingLinks(html: string): string[] {
  const links: string[] = [];
  const seen = new Set<string>();
  for (const match of html.matchAll(LINK_PATTERN)) {
    const url = match[1];
    if (!url.startsWith("http")) continue;
    if (url.length > MAX_LINK_LENGTH) continue;
    if (!INTERESTING_LINK.test(url)) continue;
    if (seen.has(url)) continue;
    seen.add(url);
    links.push(url);
    if (links.length >= MAX_LINKS) break;
  }
  return links;
}

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
