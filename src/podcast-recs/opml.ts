import type { PodcastSubscription } from "./account.js";

/** Matches any `<outline ...>` start tag (self-closing or not), any nesting depth. */
const OUTLINE_TAG_RE = /<outline\b([^>]*)>/gi;
/** Matches `name="value"` or `name='value'` attribute pairs within a tag's attribute string. */
const ATTR_RE = /([a-zA-Z_:][-a-zA-Z0-9_:.]*)\s*=\s*("([^"]*)"|'([^']*)')/g;

const NAMED_XML_ENTITIES: Record<string, string> = {
  amp: "&",
  lt: "<",
  gt: ">",
  quot: '"',
  apos: "'",
};

/** Decodes the standard XML entities (named + numeric) that may appear in attribute values. */
function decodeXmlEntities(value: string): string {
  return value.replace(
    /&(#x[0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);/g,
    (match, entity: string) => {
      if (entity.startsWith("#")) {
        const isHex = entity[1] === "x" || entity[1] === "X";
        const codePoint = Number.parseInt(entity.slice(isHex ? 2 : 1), isHex ? 16 : 10);
        if (Number.isNaN(codePoint)) return match;
        try {
          return String.fromCodePoint(codePoint);
        } catch {
          return match;
        }
      }
      return NAMED_XML_ENTITIES[entity] ?? match;
    },
  );
}

/** Extracts and decodes attribute name/value pairs from a raw `<tag ...>` attribute string. */
function parseAttributes(attrString: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  const attrRe = new RegExp(ATTR_RE.source, ATTR_RE.flags);
  let match: RegExpExecArray | null;
  // biome-ignore lint/suspicious/noAssignInExpressions: standard regex-exec loop
  while ((match = attrRe.exec(attrString)) !== null) {
    const name = match[1].toLowerCase();
    const rawValue = match[3] !== undefined ? match[3] : (match[4] ?? "");
    attrs[name] = decodeXmlEntities(rawValue);
  }
  return attrs;
}

/** Falls back to a feed URL's host when no title/text attribute is present. */
function hostFromFeedUrl(feedUrl: string): string {
  try {
    return new URL(feedUrl).host || feedUrl;
  } catch {
    return feedUrl;
  }
}

/**
 * Parses podcast OPML exports (Castro/Overcast/Apple style) into subscriptions.
 *
 * Pure and permissive: scans for `<outline>` tags at any nesting depth and
 * keeps only those carrying an `xmlUrl` attribute (folder/group outlines are
 * skipped). Malformed or non-OPML input yields an empty array rather than
 * throwing.
 */
export function parseOpmlSubscriptions(xml: string): PodcastSubscription[] {
  if (typeof xml !== "string" || xml.length === 0) return [];

  const subscriptions: PodcastSubscription[] = [];
  const seenFeedUrls = new Set<string>();

  const outlineRe = new RegExp(OUTLINE_TAG_RE.source, OUTLINE_TAG_RE.flags);
  let match: RegExpExecArray | null;
  // biome-ignore lint/suspicious/noAssignInExpressions: standard regex-exec loop
  while ((match = outlineRe.exec(xml)) !== null) {
    const attrs = parseAttributes(match[1]);
    const feedUrl = attrs.xmlurl;
    if (!feedUrl || seenFeedUrls.has(feedUrl)) continue;
    seenFeedUrls.add(feedUrl);

    const title = attrs.text || attrs.title || hostFromFeedUrl(feedUrl);
    subscriptions.push({ title, feedUrl });
  }

  return subscriptions;
}
