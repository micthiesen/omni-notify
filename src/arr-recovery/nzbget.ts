import { Effect, Schema } from "effect";
import { readFetchResponseTextWithLimit } from "../effect/publicHttp.js";
import { ArrRecoveryError, type ArrKind } from "./types.js";

const Parameters = Schema.Array(
  Schema.Struct({ Name: Schema.String, Value: Schema.String }),
);
const QueueSchema = Schema.Struct({
  result: Schema.Array(Schema.Struct({ NZBID: Schema.Number, Parameters })),
});
const HistorySchema = Schema.Struct({
  result: Schema.Array(
    Schema.Struct({
      NZBID: Schema.Number,
      Status: Schema.String,
      Category: Schema.String,
      DestDir: Schema.String,
      FinalDir: Schema.String,
      Parameters,
    }),
  ),
});

/** Corroborates Arr's no-files rejection without treating a parked or active job as a completed download. */
export function downloadHealth(
  url: string,
  kind: ArrKind,
  downloadId: string,
  outputPath: string,
  fetchImpl: typeof fetch = fetch,
) {
  const rpc = <A, I>(method: string, params: unknown[], schema: Schema.Codec<A, I>) =>
    Effect.gen(function* () {
      const response = yield* Effect.tryPromise({
        try: (signal) =>
          fetchImpl(new URL("jsonrpc", `${url.replace(/\/$/, "")}/`), {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              "User-Agent": "OpenAI File Downloader, XaiImageApiFetch/1.0",
            },
            body: JSON.stringify({ method, params, id: 1 }),
            signal,
          }),
        catch: () =>
          new ArrRecoveryError({
            operation: `NZBGet ${method}`,
            cause: "Request failed",
          }),
      });
      if (!response.ok)
        return yield* new ArrRecoveryError({
          operation: `NZBGet ${method}`,
          cause: `HTTP ${response.status}`,
        });
      const text = yield* Effect.tryPromise({
        try: (signal) =>
          readFetchResponseTextWithLimit(response, 8 * 1024 * 1024, signal),
        catch: () =>
          new ArrRecoveryError({
            operation: `NZBGet ${method}`,
            cause: "Invalid response",
          }),
      });
      const raw = yield* Effect.try({
        try: () => JSON.parse(text),
        catch: () =>
          new ArrRecoveryError({
            operation: `NZBGet ${method}`,
            cause: "Invalid JSON",
          }),
      });
      return yield* Schema.decodeUnknownEffect(schema)(raw).pipe(
        Effect.mapError(
          () =>
            new ArrRecoveryError({
              operation: `NZBGet ${method}`,
              cause: "Unexpected response schema",
            }),
        ),
      );
    });
  const matches = (item: { Parameters: readonly { Name: string; Value: string }[] }) =>
    item.Parameters.some((p) => p.Name === "drone" && p.Value === downloadId);
  return Effect.gen(function* () {
    const active = yield* rpc("listgroups", [], QueueSchema);
    if (active.result.some(matches)) return undefined;
    const history = yield* rpc("history", [false], HistorySchema);
    const items = history.result.filter(matches);
    if (items.length !== 1) return undefined;
    const item = items[0];
    if (item.Category !== kind || (item.FinalDir || item.DestDir) !== outputPath)
      return undefined;
    if (
      !/^(?:WARNING\/HEALTH|FAILURE\/(?:HEALTH|PAR|UNPACK|PASSWORD))$/.test(item.Status)
    )
      return undefined;
    // Fetch active work again: a retry may have started during the history read.
    if ((yield* rpc("listgroups", [], QueueSchema)).result.some(matches))
      return undefined;
    return item.Status;
  }).pipe(
    Effect.timeout("20 seconds"),
    Effect.mapError(
      (cause) =>
        new ArrRecoveryError({ operation: "confirm terminal download failure", cause }),
    ),
  );
}
