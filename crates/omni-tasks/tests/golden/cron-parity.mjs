// Regenerate from the repository root (so `effect` resolves from node_modules):
//   cp crates/omni-tasks/tests/golden/cron-parity.mjs ./cron-parity.tmp.mjs &&
//   TZ=America/Vancouver node ./cron-parity.tmp.mjs > crates/omni-tasks/tests/golden/cron-parity.json &&
//   rm ./cron-parity.tmp.mjs
// Generates Effect Cron.next sequences (process TZ = America/Vancouver, no explicit tz,
// exactly like mitools Scheduler) for every task-map schedule.
import { Cron, Result } from "effect";
const schedules = [
  "*/20 * * * * *", "0 */10 * * * *", "0 */5 * * * *", "0 */15 * * * *",
  "0 * * * * *", "0 */6 * * *", "0 0 11 * * 1,3,5", "0 0 5 * * 0",
  "0 0 17 * * 1,3,5", "0 0 4 * * 0", "0 0 9 * * 0", "*/5 * * * *",
  "0 0 */6 * * *", "*/30 * * * * *", "*/15 * * * * *", "*/15 * * * *", "0 17 * * * *",
  "0 30 2 * * *", "0 30 1 * * *",
];
// Windows: a full year for slow schedules; DST-adjacent windows for fast ones.
const year = [Date.UTC(2026, 0, 1, 8), Date.UTC(2027, 0, 1, 8)];
const dstWindows = [
  [Date.UTC(2026, 2, 8, 8, 30), Date.UTC(2026, 2, 8, 11, 30)],   // 00:30-03:30 PST spring forward
  [Date.UTC(2026, 10, 1, 7, 30), Date.UTC(2026, 10, 1, 11, 30)], // 00:30-03:30 PDT fall back
];
const out = {};
for (const expr of schedules) {
  const parsed = Cron.parse(expr);
  if (Result.isFailure(parsed)) throw new Error(expr);
  const cron = parsed.success;
  const windows = [];
  const fast = /^\*\/\d+ \* \* \* \* \*$|^0 \* \* \* \* \*$|^0 \*\/(5|10|15) \* \* \* \*$|^\*\/\d+ \* \* \* \*$|^0 17 \* \* \* \*$/.test(expr);
  for (const [start, end] of fast ? dstWindows : [year]) {
    const seq = [];
    let t = start;
    for (;;) {
      const next = Cron.next(cron, t).getTime();
      if (next > end) break;
      seq.push(next);
      t = next;
    }
    windows.push({ start, end, next: seq });
  }
  out[expr] = windows;
}
process.stdout.write(JSON.stringify(out));
