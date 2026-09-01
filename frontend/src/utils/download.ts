import { Effect } from "effect";

/** Trigger a client-side file download of in-memory content. */
export function downloadFile(
  filename: string,
  content: string,
  mimeType: string,
): Effect.Effect<void> {
  return Effect.gen(function* () {
    const url = yield* Effect.sync(() =>
      URL.createObjectURL(new Blob([content], { type: mimeType })),
    );
    yield* Effect.sync(() => {
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = filename;
      anchor.click();
    });
    yield* Effect.sleep("10 seconds");
    yield* Effect.sync(() => URL.revokeObjectURL(url));
  });
}
