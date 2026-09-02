import { runMain } from "@micthiesen/mitools/cli";
import { exponentialBackoff } from "@micthiesen/mitools/async";
import { Entity } from "@micthiesen/mitools/entities";
import { Logger, type NamedLogger } from "@micthiesen/mitools/logging";
import { Scheduler, type ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Effect } from "effect";
import { BriefingAgentTask } from "./briefing-agent/BriefingAgentTask.js";
import { loadBriefingConfigs } from "./briefing-agent/configs.js";
import { createCalendarHandler } from "./calendar-events/index.js";
import { importHistoricalCosts } from "./costs/migrate.js";
import { EmailDispatcher } from "./email/dispatcher.js";
import { ImapTransport } from "./email/imap/transport.js";
import EmailRetryTask from "./email/retryTask.js";
import { EmailTriageService } from "./email/triage.js";
import type { EmailHandler } from "./email/types.js";
import EmailWatchdogTask from "./email/watchdogTask.js";
import { IntegrationError } from "./effect/errors.js";
import { AppLayer, type AppServices, runnerFromContext } from "./effect/appRuntime.js";
import { ApnsControlClient } from "./ios-controls/apns.js";
import { IOSControlService } from "./ios-controls/service.js";
import { loadChannelsConfig } from "./live-check/channelsConfig.js";
import {
  createLivestreamIntelligenceService,
  type LivestreamIntelligenceObserver,
} from "./live-check/intelligence/service.js";
import { Platform } from "./live-check/platforms/index.js";
import {
  buildStreamers,
  dropPlatformBindings,
  type Streamer,
} from "./live-check/streamers.js";
import LiveCheckTask from "./live-check/task.js";
import { createParcelHandler } from "./parcel-tracker/index.js";
import PetTrackerTask from "./pet-tracker/task.js";
import { CastroInboxCleanupTask } from "./podcast-recs/castro/inboxCleanupTask.js";
import { PodcastTasteReflectionTask } from "./podcast-recs/reflection/index.js";
import { PodcastRecommendationTask } from "./podcast-recs/task.js";
import PressPodsTask from "./press-pods/task.js";
import { MediaRecommendationTask } from "./recommendations/task.js";
import { MediaTasteReflectionTask } from "./recommendations/taste/task.js";
import { type EmailControls, startServer } from "./server.js";
import { TaskRegistry, type TaskServices } from "./task-runs/registry.js";
import config, { logLoadedConfig } from "./utils/config.js";
import { workspaceDefinitions } from "./workspaces/definitions.js";
import { WorkspaceEmailHandler } from "./workspaces/email.js";
import { WorkspaceOperationError } from "./workspaces/errors.js";
import { WorkspaceNotificationTask } from "./workspaces/notifications.js";
import { WorkspaceTask } from "./workspaces/task.js";

const LEGACY_CHANNEL_ENV_VARS = [
  "YT_CHANNEL_NAMES",
  "TWITCH_CHANNEL_NAMES",
  "KICK_CHANNEL_NAMES",
] as const;
const LEGACY_EMAIL_ENV_VARS = [
  "EMAIL_TRANSPORT",
  "CALDAV_PROVIDER",
  "FASTMAIL_API_TOKEN",
  "FASTMAIL_APP_PASSWORD",
  "FASTMAIL_USERNAME",
  "FASTMAIL_CALENDAR_ID",
] as const;

const loadStreamers = Effect.fn("Main.loadStreamers")(function* (logger: NamedLogger) {
  for (const key of LEGACY_CHANNEL_ENV_VARS) {
    if (process.env[key]) {
      yield* logger.warn(
        `${key} is no longer read, channels are configured in channels.json`,
      );
    }
  }
  for (const key of LEGACY_EMAIL_ENV_VARS) {
    if (process.env[key]) {
      yield* logger.warn(
        `${key} is no longer read, email and calendar use iCloud credentials`,
      );
    }
  }
  const liveCheckConfig = loadChannelsConfig();
  const streamers = buildStreamers(liveCheckConfig.channels);
  const availablePlatforms = new Set(Object.values(Platform));
  if (config.KICK_CLIENT_ID && config.KICK_CLIENT_SECRET) {
    return {
      streamers,
      dggTopEmbeds: liveCheckConfig.dggTopEmbeds,
      availablePlatforms,
    };
  }
  const { streamers: withoutKick, droppedAny } = dropPlatformBindings(
    streamers,
    Platform.Kick,
  );
  if (droppedAny) {
    yield* logger.warn(
      "Kick channels configured but KICK_CLIENT_ID/KICK_CLIENT_SECRET missing; skipping Kick",
    );
    const remaining = new Set(withoutKick.map((streamer) => streamer.id));
    const fullyDropped = streamers
      .filter((streamer) => !remaining.has(streamer.id))
      .map((streamer) => streamer.displayName);
    if (fullyDropped.length > 0) {
      yield* logger.warn(`Not tracked at all (Kick-only): ${fullyDropped.join(", ")}`);
    }
  }
  return {
    streamers: withoutKick,
    dggTopEmbeds: liveCheckConfig.dggTopEmbeds,
    availablePlatforms,
  };
});

const createIOSControlService = Effect.fn("Main.createIOSControlService")(function* (
  streamers: Streamer[],
  logger: NamedLogger,
) {
  const apnsValues = [
    config.IOS_CONTROL_APNS_TEAM_ID,
    config.IOS_CONTROL_APNS_KEY_ID,
    config.IOS_CONTROL_APNS_KEY_PATH,
  ];
  const hasAnyApnsConfig = apnsValues.some(Boolean);
  const hasCompleteApnsConfig = apnsValues.every(Boolean);
  if (hasAnyApnsConfig && !hasCompleteApnsConfig) {
    yield* logger.warn(
      "iOS control APNs pushes disabled: team ID, key ID, and key path must all be set",
    );
  }
  let apns: ApnsControlClient | undefined;
  if (hasCompleteApnsConfig && config.IOS_CONTROL_AUTH_TOKEN) {
    apns = yield* ApnsControlClient.createEffect({
      teamId: config.IOS_CONTROL_APNS_TEAM_ID as string,
      keyId: config.IOS_CONTROL_APNS_KEY_ID as string,
      privateKeyPath: config.IOS_CONTROL_APNS_KEY_PATH as string,
      bundleId: config.IOS_CONTROL_BUNDLE_ID,
    }).pipe(
      Effect.catch((error) =>
        logger
          .warn(
            `iOS control APNs pushes disabled: failed to load signing key (${error.message})`,
          )
          .pipe(Effect.as(undefined)),
      ),
    );
  }
  if (hasCompleteApnsConfig && !config.IOS_CONTROL_AUTH_TOKEN) {
    yield* logger.warn(
      "iOS control APNs pushes disabled: IOS_CONTROL_AUTH_TOKEN is not set",
    );
  }
  const service = new IOSControlService(
    streamers,
    config.IOS_CONTROL_HOME_URL,
    logger,
    apns,
  );
  yield* Effect.addFinalizer(() => Effect.sync(() => service.close()));
  return service;
});

const buildTasks = Effect.fn("Main.buildTasks")(function* (
  streamers: Streamer[],
  logger: NamedLogger,
  iosControls: IOSControlService,
  dggTopEmbeds: number,
  availablePlatforms: ReadonlySet<Platform>,
  intelligence: LivestreamIntelligenceObserver | undefined,
) {
  const tasks: ScheduledTask<unknown, TaskServices>[] = [];
  if (streamers.length > 0 || dggTopEmbeds > 0) {
    tasks.push(
      new LiveCheckTask(
        streamers,
        logger,
        () => iosControls.reconcileEffect(),
        dggTopEmbeds > 0 ? { topEmbeds: dggTopEmbeds, availablePlatforms } : undefined,
        intelligence,
      ),
    );
  }
  if (config.WHISKER_CREDENTIALS) {
    tasks.push(new PetTrackerTask(config.WHISKER_CREDENTIALS, logger));
  }
  for (const briefingConfig of yield* loadBriefingConfigs(logger)) {
    const task = yield* BriefingAgentTask.create(briefingConfig, logger);
    if (task) tasks.push(task);
  }
  const optionalTasks = yield* Effect.all([
    PressPodsTask.create(logger),
    MediaRecommendationTask.create(logger),
    PodcastRecommendationTask.create(logger),
    CastroInboxCleanupTask.create(logger),
    MediaTasteReflectionTask.create(logger),
    PodcastTasteReflectionTask.create(logger),
  ]);
  for (const task of optionalTasks) if (task) tasks.push(task);
  for (const definition of workspaceDefinitions) {
    tasks.push(new WorkspaceTask(definition, logger));
  }
  tasks.push(new WorkspaceNotificationTask(logger));
  return tasks;
});

const startEmailFeatures = Effect.fn("Main.startEmailFeatures")(function* (
  parentLogger: NamedLogger,
  requestWorkspaceEmailRun: (
    workspaceId: string,
    subjectId: string,
    message: string,
    trigger: "email",
  ) => Effect.Effect<void, WorkspaceOperationError, TaskServices>,
) {
  if (!config.ICLOUD_USERNAME || !config.ICLOUD_APP_PASSWORD) return undefined;
  const emailLogger = parentLogger.extend("Email");
  const transport = new ImapTransport(
    { user: config.ICLOUD_USERNAME, pass: config.ICLOUD_APP_PASSWORD },
    parentLogger.extend("IMAP"),
  );
  const dispatcher = new EmailDispatcher<TaskServices>(transport, emailLogger);
  const triage = new EmailTriageService(emailLogger.extend("Triage"));
  const parcel = yield* createParcelHandler(parentLogger, triage);
  if (parcel) dispatcher.register(parcel);
  const calendar = yield* createCalendarHandler(transport, parentLogger, triage);
  if (calendar) dispatcher.register(calendar);
  const workspaces = new WorkspaceEmailHandler(
    requestWorkspaceEmailRun,
    emailLogger.extend("Workspaces"),
  );
  dispatcher.register(workspaces);
  if (dispatcher.handlerCount === 0) {
    yield* emailLogger.info("No email pipelines active");
    return undefined;
  }
  const handlers = new Map<string, EmailHandler<unknown, TaskServices>>();
  if (parcel) handlers.set(parcel.name, parcel);
  if (calendar) handlers.set(calendar.name, calendar);
  handlers.set(workspaces.name, workspaces);
  yield* dispatcher.startEffect.pipe(
    Effect.mapError(
      (cause) => new IntegrationError({ operation: "start email transport", cause }),
    ),
  );
  yield* emailLogger.info(
    `Started ${transport.name} transport with ${dispatcher.handlerCount} pipeline(s)`,
  );
  return { cleanupEffect: dispatcher.stopEffect, transport, handlers };
});

const program = Effect.scoped(
  Effect.gen(function* () {
    const logger = Logger.named("Main");
    yield* logLoadedConfig;
    const migrated = yield* Entity.migrateAll();
    if (migrated > 0) yield* logger.info(`Migrated ${migrated} entity row(s)`);
    const imported = yield* importHistoricalCosts();
    if (imported > 0) {
      yield* logger.info(`Imported ${imported} historical cost event(s)`);
    }
    const intelligence = yield* createLivestreamIntelligenceService(logger);
    if (intelligence) yield* Effect.addFinalizer(() => intelligence.close());
    const loaded = yield* loadStreamers(logger);
    const iosControls = yield* createIOSControlService(loaded.streamers, logger);
    const tasks = yield* buildTasks(
      loaded.streamers,
      logger,
      iosControls,
      loaded.dggTopEmbeds,
      loaded.availablePlatforms,
      intelligence,
    );

    const runTaskIndex = process.argv.indexOf("--run-task");
    if (runTaskIndex !== -1) {
      const taskName = process.argv[runTaskIndex + 1];
      if (!taskName) {
        yield* logger.error("Usage: --run-task <TaskName>");
        return 1;
      }
      const task = tasks.find(
        (candidate) => candidate.name.toLowerCase() === taskName.toLowerCase(),
      );
      if (!task) {
        yield* logger.error(
          `Unknown task "${taskName}". Available: ${tasks.map((item) => item.name).join(", ")}`,
        );
        return 1;
      }
      yield* logger.info(`Running task "${task.name}" once...`);
      yield* task.run;
      yield* logger.info(`Task "${task.name}" complete`);
      return;
    }

    const context = yield* Effect.context<AppServices>();
    const effectRunner = runnerFromContext(context);
    const registry = new TaskRegistry(logger);
    yield* registry.initializeEffect();
    yield* Effect.addFinalizer(() => registry.shutdownEffect());
    const emailControls: EmailControls = {};
    const requestWorkspaceEmailRun = (
      workspaceId: string,
      subjectId: string,
      message: string,
      trigger: "email" = "email",
    ) => {
      const definition = workspaceDefinitions.find((item) => item.id === workspaceId);
      if (!definition) {
        return Effect.fail(
          new WorkspaceOperationError({
            operation: "resolve email-triggered workspace",
            cause: new Error(`Unknown workspace ${workspaceId}`),
          }),
        );
      }
      return registry
        .runNowAndWaitEffect(definition.taskName, { message, subjectId, trigger })
        .pipe(
          Effect.tap(() =>
            logger.info(
              `Completed workspace email run for ${workspaceId}/${subjectId}`,
            ),
          ),
          Effect.mapError(
            (cause) =>
              new WorkspaceOperationError({
                operation: "run email-triggered workspace",
                cause,
              }),
          ),
          Effect.asVoid,
        );
    };
    const closeServer = startServer(
      config.FRONTEND_PORT,
      logger,
      effectRunner,
      registry,
      loaded.streamers,
      emailControls,
      iosControls,
      intelligence,
    );
    yield* Effect.addFinalizer(() =>
      closeServer.pipe(
        Effect.catch((error) => logger.error("HTTP server shutdown failed", error)),
      ),
    );

    if (process.argv.includes("--server-only")) {
      yield* logger.info("Running in server-only mode (tasks disabled)");
      return yield* Effect.never;
    }
    const scheduler = yield* Scheduler;
    for (const task of tasks) yield* scheduler.register(registry.track(task));
    let emailCleanup: Effect.Effect<void, never, TaskServices> = Effect.void;
    if (config.ICLOUD_USERNAME && config.ICLOUD_APP_PASSWORD) {
      yield* scheduler.register(registry.track(new EmailWatchdogTask(logger)));
      yield* scheduler.register(
        registry.track(new EmailRetryTask(() => emailControls, logger)),
      );
      yield* Effect.addFinalizer(() => emailCleanup);
      yield* startEmailFeatures(logger, requestWorkspaceEmailRun).pipe(
        Effect.tap((email) =>
          Effect.sync(() => {
            if (!email) return;
            emailCleanup = email.cleanupEffect;
            emailControls.transport = email.transport;
            emailControls.handlers = email.handlers;
          }),
        ),
        Effect.tapError((error) =>
          logger.error("Failed to start email features, retrying", error),
        ),
        Effect.retry(exponentialBackoff({ baseDelayMs: 30_000, maxDelayMs: 300_000 })),
        Effect.forkScoped,
      );
    } else {
      yield* logger.info(
        "Email features disabled: no transport configured (ICLOUD_USERNAME + ICLOUD_APP_PASSWORD)",
      );
    }
    yield* scheduler.start;
    yield* registry.recoverMissedTasksEffect().pipe(
      Effect.catchCause((cause) =>
        logger.error("Failed to recover missed task runs", cause),
      ),
      Effect.forkScoped,
    );
    return yield* Effect.never;
  }),
).pipe(Effect.ensuring(Logger.flush), Effect.provide(AppLayer));

runMain(program, { debug: process.env.OMNI_DEBUG !== undefined });
