const QUOTE_PREFIX_RE = /^\s*[>]+\s*/i;

export const removeExtraEmptyLines = (lines: string[]): string[] => {
  return lines.filter((line, index) => {
    const infoCurr = getLineInfo(line);
    if (!infoCurr.isEmpty) return true;

    if (infoCurr.isQuote) return false; // Empty quote lines are never wanted

    const infoPrev = getLineInfo(lines[index - 1]);
    const infoNext = getLineInfo(lines[index + 1]);

    if (!infoPrev.exists) return false;
    if (!infoNext.exists) return false;

    if (!infoCurr.isQuote && !infoPrev.isQuote && infoNext.isEmpty) return false;

    return true;
  });
};

const getLineInfo = (line: string | undefined) => {
  if (line === undefined) return { isQuote: false, isEmpty: false, exists: false };

  const isQuote = QUOTE_PREFIX_RE.test(line);
  const lineClean = line.replace(QUOTE_PREFIX_RE, "");
  const isEmpty = lineClean.length === 0;
  return { isQuote, isEmpty, exists: true };
};
