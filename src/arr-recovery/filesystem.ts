import { readdir } from "node:fs/promises";
import { posix } from "node:path";
import { Effect } from "effect";
import { ArrRecoveryError } from "./types.js";

/** The optional read-only mounts let us distinguish an empty directory from an inaccessible one. */
export function verifyDownloadRemoved(outputPath: string) {
  const normalized = posix.normalize(outputPath);
  const roots = ["/media/storage/nzbget/completed", "/tmp/inter"];
  if (!roots.some((root) => normalized.startsWith(`${root}/`))) {
    return Effect.fail(
      new ArrRecoveryError({
        operation: "verify download deletion",
        cause: "Path is outside the read-only download mounts",
      }),
    );
  }
  return Effect.tryPromise({
    try: () => readdir(posix.dirname(normalized)),
    catch: () =>
      new ArrRecoveryError({
        operation: "verify download deletion",
        cause: "Download parent directory could not be read",
      }),
  }).pipe(Effect.map((entries) => !entries.includes(posix.basename(normalized))));
}
