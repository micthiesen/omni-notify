/** Wrap content in a fenced code block, using a fence that won't collide with the content. */
export function codeBlock(content: string, lang?: string): string {
  let fence = "```";
  while (content.includes(fence)) fence += "`";
  return `${fence}${lang ?? ""}\n${content}\n${fence}`;
}
