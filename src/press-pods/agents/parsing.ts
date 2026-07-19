export function extractBetweenTags(text: string, tag: string): string {
  const regex = new RegExp(`<${tag}>([\\s\\S]*?)<\\/${tag}>`);
  const extractedContent = text.match(regex)?.[1]?.trim();
  if (!extractedContent) {
    throw new Error(`Failed to extract content between <${tag}> tags`);
  }
  return extractedContent;
}
