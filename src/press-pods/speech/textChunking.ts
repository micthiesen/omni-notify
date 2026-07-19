export interface NarrationSection {
  /** Section heading (from a `## Heading` line), or undefined for the intro. */
  title: string | undefined;
  body: string;
}

const SECTION_HEADING_RE = /^##\s+(.+?)\s*$/;

/**
 * Split narration into sections on `## Heading` lines (the cleaner emits one
 * per major article section). Text before the first heading is the untitled
 * intro section. Heading lines are consumed here — they mark chunk boundaries
 * and become chapter titles, but are never sent to TTS.
 */
export function splitSections(text: string): NarrationSection[] {
  const sections: NarrationSection[] = [];
  let title: string | undefined;
  let buf: string[] = [];
  const flush = (): void => {
    const body = buf.join("\n").trim();
    if (body.length > 0) sections.push({ title, body });
    buf = [];
  };
  for (const line of text.split("\n")) {
    const match = line.match(SECTION_HEADING_RE);
    if (match) {
      flush();
      title = match[1].trim();
    } else {
      buf.push(line);
    }
  }
  flush();
  return sections.length > 0 ? sections : [{ title: undefined, body: text.trim() }];
}

function splitSentences(paragraph: string): string[] {
  return paragraph.split(/(?<=[.!?…])\s+/).filter((s) => s.trim().length > 0);
}

/**
 * Break section body into TTS-sized chunks on paragraph boundaries (falling
 * back to sentence boundaries for oversized paragraphs). Never splits
 * mid-sentence: a mid-sentence cut makes the model generate two terminal
 * prosody contours and a spurious pause at the seam.
 */
export function chunkText(text: string, target: number, max: number): string[] {
  const paragraphs = text
    .split(/\n\s*\n/)
    .map((p) => p.trim())
    .filter((p) => p.length > 0);

  const units: string[] = [];
  for (const paragraph of paragraphs) {
    if (paragraph.length <= max) {
      units.push(paragraph);
      continue;
    }
    let buf = "";
    for (const sentence of splitSentences(paragraph)) {
      if (buf && buf.length + sentence.length + 1 > target) {
        units.push(buf);
        buf = sentence;
      } else {
        buf = buf ? `${buf} ${sentence}` : sentence;
      }
    }
    if (buf) units.push(buf);
  }

  const chunks: string[] = [];
  let buf = "";
  for (const unit of units) {
    if (buf && buf.length + unit.length + 2 > target) {
      chunks.push(buf);
      buf = unit;
    } else {
      buf = buf ? `${buf}\n\n${unit}` : unit;
    }
  }
  if (buf) chunks.push(buf);
  return chunks;
}
