/** Generate a filesystem-safe timestamp for log filenames, e.g. "2026-03-16T14-30-05". */
export function logTimestamp(): string {
  const d = new Date();
  const pad = (n: number) => n.toString().padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}-${pad(d.getMinutes())}-${pad(d.getSeconds())}`;
}

/** Wrap content in a fenced code block, using a fence that won't collide with the content. */
export function codeBlock(content: string, lang?: string): string {
  let fence = "```";
  while (content.includes(fence)) fence += "`";
  return `${fence}${lang ?? ""}\n${content}\n${fence}`;
}
