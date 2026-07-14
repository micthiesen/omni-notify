import { formatClockTime, pad2 } from "./format";

const DOW_NAMES = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const DOW_ALIASES: Record<string, number> = {
  sun: 0,
  mon: 1,
  tue: 2,
  wed: 3,
  thu: 4,
  fri: 5,
  sat: 6,
};

function isStar(field: string): boolean {
  return field === "*";
}

function stepOf(field: string): number | null {
  const match = /^\*\/(\d+)$/.exec(field);
  return match ? Number(match[1]) : null;
}

function numOf(field: string): number | null {
  return /^\d+$/.test(field) ? Number(field) : null;
}

function dowIndex(token: string): number | null {
  const asNum = numOf(token);
  if (asNum !== null) return asNum === 7 ? 0 : asNum <= 6 ? asNum : null;
  const alias = DOW_ALIASES[token.slice(0, 3).toLowerCase()];
  return alias !== undefined ? alias : null;
}

function describeDow(field: string): string | null {
  const parts = field.split(",");
  const names: string[] = [];
  for (const part of parts) {
    const rangeMatch = /^([^-]+)-([^-]+)$/.exec(part);
    if (rangeMatch) {
      const from = dowIndex(rangeMatch[1]);
      const to = dowIndex(rangeMatch[2]);
      if (from === null || to === null) return null;
      names.push(`${DOW_NAMES[from]}-${DOW_NAMES[to]}`);
    } else {
      const idx = dowIndex(part);
      if (idx === null) return null;
      names.push(DOW_NAMES[idx]);
    }
  }
  return names.join(", ");
}

/**
 * Best-effort human rendering of a 6-field cron expression
 * ("sec min hour dom mon dow"). Returns null for anything it
 * can't confidently describe; callers should fall back to raw cron.
 */
export function describeCron(expr: string): string | null {
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 6) return null;
  const [sec, min, hour, dom, mon, dow] = parts;

  const restIsStar =
    isStar(hour) && isStar(dom) && isStar(mon) && isStar(dow);

  const secStep = stepOf(sec);
  if (secStep !== null && isStar(min) && restIsStar) {
    return secStep === 1 ? "every second" : `every ${secStep} seconds`;
  }

  const secNum = numOf(sec);
  if (secNum === null) return null;

  const minStep = stepOf(min);
  if (minStep !== null && restIsStar) {
    return minStep === 1 ? "every minute" : `every ${minStep} minutes`;
  }
  if (isStar(min) && restIsStar) return "every minute";

  const minNum = numOf(min);
  if (minNum === null) return null;

  const dateIsStar = isStar(dom) && isStar(mon) && isStar(dow);

  const hourStep = stepOf(hour);
  if (hourStep !== null && dateIsStar) {
    return hourStep === 1
      ? `hourly at :${pad2(minNum)}`
      : `every ${hourStep} hours at :${pad2(minNum)}`;
  }
  if (isStar(hour) && dateIsStar) return `hourly at :${pad2(minNum)}`;

  const hourNum = numOf(hour);
  if (hourNum === null) return null;
  const time = formatClockTime(hourNum, minNum);

  if (dateIsStar) return `daily at ${time}`;

  if (isStar(dom) && isStar(mon)) {
    const days = describeDow(dow);
    return days ? `${days} at ${time}` : null;
  }

  const domNum = numOf(dom);
  if (domNum !== null && isStar(mon) && isStar(dow)) {
    return `monthly on day ${domNum} at ${time}`;
  }

  return null;
}
