/**
 * Extract the text a model wrapped in `<tag>…</tag>`.
 *
 * Autoregressive models occasionally truncate before the closing tag, mangle
 * its casing, or start a partial `</tag` fragment. We prefer a well-formed
 * pair, then fall back to an unclosed opening tag (taking everything after it
 * and trimming any dangling close fragment) before giving up.
 */
export function extractBetweenTags(text: string, tag: string): string {
  const paired = text
    .match(new RegExp(`<${tag}>([\\s\\S]*?)</${tag}>`, "i"))?.[1]
    ?.trim();
  if (paired) return paired;

  // Unclosed opening tag: keep everything after it, dropping any partial
  // closing fragment the model began (e.g. a trailing "</cleaned_artic").
  const openMatch = text.match(new RegExp(`<${tag}>([\\s\\S]*)$`, "i"));
  if (openMatch) {
    const tail = openMatch[1]
      .replace(new RegExp(`</?${tag}[^>]*>?\\s*$`, "i"), "")
      .trim();
    if (tail) return tail;
  }

  throw new Error(`Failed to extract content between <${tag}> tags`);
}
