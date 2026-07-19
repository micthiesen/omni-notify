export const USER_AGENT =
  "Mozilla/5.0 (iPhone; CPU iPhone OS 14_0 like Mac OS X) AppleWebKit/605.1.15" +
  " (KHTML, like Gecko) Version/14.0 Mobile/15A372 Safari/604.1";

export function extractTitleFromHtml(html: string): string | undefined {
  const match = html.match(/<title>([^<]*)<\/title>/i);
  return match?.[1]?.trim() || undefined;
}
