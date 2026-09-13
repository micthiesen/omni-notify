import type { NamedLogger } from "@micthiesen/mitools/logging";
import { notify } from "@micthiesen/mitools/pushover";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import { ObserverClient } from "../observer/client.js";
import type { TaskServices } from "../task-runs/registry.js";
import config from "../utils/config.js";
import { assessIssue, ObserverRepairError } from "./agent.js";
import { ArrRepairClient } from "./arr.js";
import { type RepairDependencies, runObserverRepair } from "./service.js";

export function repairDependencies(
  observer: ObserverClient,
  clients: Partial<Record<"tv" | "movie", ArrRepairClient>>,
): RepairDependencies<unknown, TaskServices> {
  const clientFor = (mediaType: string | null | undefined) => {
    const client =
      mediaType === "tv" || mediaType === "movie" ? clients[mediaType] : undefined;
    return client
      ? Effect.succeed(client)
      : Effect.fail(
          new ObserverRepairError({
            operation: "select configured Arr client",
            cause: "Unsupported media or missing Arr configuration",
          }),
        );
  };
  return {
    listOpen: () => observer.listIssues({ filter: "open", maxRecords: 100 }),
    getIssue: (id: number) => observer.getIssue(id),
    assess: (issue: import("../observer/client.js").ObserverIssue) =>
      Effect.gen(function* () {
        const client = yield* clientFor(issue.media?.mediaType);
        const inspect = client.inspect(issue.media ?? {});
        const history = observer
          .listIssues({ filter: "resolved", maxRecords: 10 })
          .pipe(
            Effect.flatMap((issues) =>
              Effect.forEach(issues, (previous) => observer.getIssue(previous.id), {
                concurrency: 2,
              }),
            ),
          );
        // Only expose compact media evidence to Luna; imported paths and history URLs are not needed.
        return yield* assessIssue(
          issue,
          inspect.pipe(
            Effect.map((current) => ({
              title: current.title,
              kind: current.kind,
              monitored: current.monitored,
              episodes: current.episodes.map((e) => ({
                season: e.seasonNumber,
                episode: e.episodeNumber,
                hasFile: e.hasFile,
                monitored: e.monitored,
              })),
              fileCount: current.files.length,
              activeDownloads: current.queue.length,
            })),
          ),
          history,
        );
      }),
    prepare: (
      issue: import("../observer/client.js").ObserverIssue,
      decision: import("./agent.js").RepairDecision,
    ) =>
      Effect.gen(function* () {
        if (decision.action === "cannot_handle")
          return yield* new ObserverRepairError({
            operation: "prepare repair",
            cause: "Cannot execute unsupported decision",
          });
        const client = yield* clientFor(issue.media?.mediaType);
        const current = yield* client.inspect(issue.media ?? {});
        const scope = yield* client.plan(current, {
          ...decision,
          action: decision.action,
        });
        return {
          summary: scope.description,
          execute: Effect.gen(function* () {
            const fresh = yield* client.inspect(issue.media ?? {});
            const checked = yield* client.plan(fresh, {
              ...decision,
              action: decision.action as "replace" | "search_missing",
            });
            if (JSON.stringify(checked) !== JSON.stringify(scope))
              return yield* new ObserverRepairError({
                operation: "revalidate Arr repair scope",
                cause: "Arr files or release mappings changed before execution",
              });
            return yield* decision.action === "replace"
              ? client.replace(checked)
              : client.search(checked);
          }),
        };
      }),
    comment: (id: number, message: string) => observer.addComment(id, message),
    resolve: (id: number) => observer.resolveIssue(id),
    send: (id: number, message: string) =>
      notify({
        title: `Observer issue #${id}`,
        message: message.slice(0, 1000),
        token: config.PUSHOVER_RECS_TOKEN,
        url: `https://media.syas.ca/issues/${id}`,
        url_title: "View issue",
      }),
  };
}

export class ObserverRepairTask implements ScheduledTask<unknown, TaskServices> {
  public readonly name = "ObserverRepair";
  public readonly displayName = "Repair Observer issues";
  public readonly schedule = "0 */15 * * * *";
  public readonly runOnStartup = true;
  private lastRunSummary?: string;
  public static create(logger: NamedLogger) {
    return Effect.gen(function* () {
      if (
        !config.OBSERVER_REPAIR_ENABLED ||
        !config.OBSERVER_URL ||
        !config.OBSERVER_API_KEY
      )
        return null;
      if (
        !config.OPENAI_API_KEY ||
        !config.PUSHOVER_USER ||
        !config.PUSHOVER_RECS_TOKEN
      ) {
        yield* logger.warn(
          "Observer repair disabled: OpenAI and Pushover credentials required",
        );
        return null;
      }
      const clients: Partial<Record<"tv" | "movie", ArrRepairClient>> = {};
      if (config.SONARR_URL && config.SONARR_API_KEY)
        clients.tv = new ArrRepairClient({
          kind: "sonarr",
          url: config.SONARR_URL,
          apiKey: config.SONARR_API_KEY,
        });
      if (config.RADARR_URL && config.RADARR_API_KEY)
        clients.movie = new ArrRepairClient({
          kind: "radarr",
          url: config.RADARR_URL,
          apiKey: config.RADARR_API_KEY,
        });
      return new ObserverRepairTask(
        logger.extend("ObserverRepair"),
        new ObserverClient({
          url: config.OBSERVER_URL,
          apiKey: config.OBSERVER_API_KEY,
        }),
        clients,
      );
    });
  }
  public constructor(
    private readonly logger: NamedLogger,
    private readonly observer: ObserverClient,
    private readonly clients: Partial<Record<"tv" | "movie", ArrRepairClient>>,
  ) {}
  public readonly run = Effect.gen({ self: this }, function* () {
    this.lastRunSummary = undefined;
    this.lastRunSummary = yield* runObserverRepair(
      repairDependencies(this.observer, this.clients),
      this.logger,
    );
  });
  public getLastRunSummary() {
    return this.lastRunSummary;
  }
}
