import type { Effect as EffectType } from "effect/Effect";
import { Injector } from "@micthiesen/mitools/config";
import { Logger } from "@micthiesen/mitools/logging";
import type { ScheduledTask } from "@micthiesen/mitools/scheduling";
import { Scheduler } from "@micthiesen/mitools/scheduling";
import { Effect, Fiber, Schedule } from "effect";
import { installAlertThrottle } from "./alerts/throttle.js";
import { BriefingAgentTask } from "./briefing-agent/BriefingAgentTask.js";
import { loadBriefingConfigs } from "./briefing-agent/configs.js";
import { createCalendarHandler } from "./calendar-events/index.js";
import { importHistoricalCosts } from "./costs/migrate.js";
import { EmailDispatcher } from "./email/dispatcher.js";
import { ImapTransport } from "./email/imap/transport.js";
import { JmapTransport } from "./email/jmap/transport.js";
import EmailRetryTask from "./email/retryTask.js";
import { EmailTriageService } from "./email/triage.js";
import type { EmailHandler, EmailTransport } from "./email/types.js";
import EmailWatchdogTask from "./email/watchdogTask.js";
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
import { installLogCapture } from "./task-runs/logCapture.js";
import { TaskRegistry } from "./task-runs/registry.js";
import config from "./utils/config.js";
import { workspaceDefinitions } from "./workspaces/definitions.js";
import { WorkspaceEmailHandler } from "./workspaces/email.js";
import { WorkspaceOperationError } from "./workspaces/errors.js";
import { WorkspaceNotificationTask } from "./workspaces/notifications.js";
import { WorkspaceTask } from "./workspaces/task.js";
import { fromPromise, runPromise } from "./effect/interop.js";
import { IntegrationError } from "./effect/errors.js";

Injector.configure({ config });
installLogCapture();
installAlertThrottle();

const logger = new Logger("Main");
const livestreamIntelligence = await runPromise(
  createLivestreamIntelligenceService(logger),
);
const importedCostEvents = importHistoricalCosts();
if (importedCostEvents > 0) {
  logger.info(`Imported ${importedCostEvents} historical cost event(s)`);
}

// channels.json is the sole source of channel config now; these env vars are
// silently ignored by utils/config.ts (dropped from its schema), so warn
// instead of leaving a stale, no-longer-honored value unexplained.
const LEGACY_CHANNEL_ENV_VARS = [
  "YT_CHANNEL_NAMES",
  "TWITCH_CHANNEL_NAMES",
  "KICK_CHANNEL_NAMES",
] as const;

function warnOnLegacyChannelEnvVars(): void {
  for (const key of LEGACY_CHANNEL_ENV_VARS) {
    if (process.env[key]) {
      logger.warn(
        `${key} is no longer read — channels are configured in channels.json`,
      );
    }
  }
}

function loadStreamers(): {
  streamers: Streamer[];
  dggTopEmbeds: number;
  availablePlatforms: Set<Platform>;
} {
  warnOnLegacyChannelEnvVars();

  const liveCheckConfig = loadChannelsConfig();
  const streamers = buildStreamers(liveCheckConfig.channels);
  const kickConfigured = Boolean(config.KICK_CLIENT_ID && config.KICK_CLIENT_SECRET);
  const availablePlatforms = new Set(Object.values(Platform));
  if (kickConfigured) {
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
    logger.warn(
      "Kick channels configured but KICK_CLIENT_ID/KICK_CLIENT_SECRET missing; skipping Kick",
    );
    // A streamer whose ONLY binding was Kick vanishes from tracking entirely —
    // name those specifically rather than letting them disappear silently.
    const remaining = new Set(withoutKick.map((s) => s.id));
    const fullyDropped = streamers
      .filter((s) => !remaining.has(s.id))
      .map((s) => s.displayName);
    if (fullyDropped.length > 0) {
      logger.warn(`Not tracked at all (Kick-only): ${fullyDropped.join(", ")}`);
    }
  }
  return {
    streamers: withoutKick,
    dggTopEmbeds: liveCheckConfig.dggTopEmbeds,
    availablePlatforms,
  };
}

function buildTasks(
  streamers: Streamer[],
  iosControls?: IOSControlService,
  dggTopEmbeds = 0,
  availablePlatforms: ReadonlySet<Platform> = new Set(Object.values(Platform)),
  intelligence: LivestreamIntelligenceObserver | undefined = livestreamIntelligence,
): ScheduledTask[] {
  const tasks: ScheduledTask[] = [];

  if (streamers.length > 0 || dggTopEmbeds > 0) {
    tasks.push(
      new LiveCheckTask(
        streamers,
        logger,
        () => iosControls?.reconcile() ?? Promise.resolve(),
        dggTopEmbeds > 0 ? { topEmbeds: dggTopEmbeds, availablePlatforms } : undefined,
        intelligence,
      ),
    );
  }
  if (config.WHISKER_CREDENTIALS) {
    tasks.push(new PetTrackerTask(config.WHISKER_CREDENTIALS, logger));
  }

  for (const config of loadBriefingConfigs(logger)) {
    const task = BriefingAgentTask.create(config, logger);
    if (task) tasks.push(task);
  }

  const pressPods = PressPodsTask.create(logger);
  if (pressPods) tasks.push(pressPods);
  const recommendations = MediaRecommendationTask.create(logger);
  if (recommendations) tasks.push(recommendations);
  const podcastRecs = PodcastRecommendationTask.create(logger);
  if (podcastRecs) tasks.push(podcastRecs);
  const castroInboxCleanup = CastroInboxCleanupTask.create(logger);
  if (castroInboxCleanup) tasks.push(castroInboxCleanup);
  const tasteReflection = MediaTasteReflectionTask.create(logger);
  if (tasteReflection) tasks.push(tasteReflection);
  const podcastTasteReflection = PodcastTasteReflectionTask.create(logger);
  if (podcastTasteReflection) tasks.push(podcastTasteReflection);
  for (const definition of workspaceDefinitions) {
    tasks.push(new WorkspaceTask(definition, logger));
  }
  tasks.push(new WorkspaceNotificationTask(logger));

  return tasks;
}

function createIOSControlServiceEffect(
  streamers: Streamer[],
): EffectType<IOSControlService> {
  const apnsValues = [
    config.IOS_CONTROL_APNS_TEAM_ID,
    config.IOS_CONTROL_APNS_KEY_ID,
    config.IOS_CONTROL_APNS_KEY_PATH,
  ];
  const hasAnyApnsConfig = apnsValues.some(Boolean);
  const hasCompleteApnsConfig = apnsValues.every(Boolean);
  return Effect.gen(function* () {
    if (hasAnyApnsConfig && !hasCompleteApnsConfig) {
      logger.warn(
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
          Effect.sync(() => {
            logger.warn(
              `iOS control APNs pushes disabled: failed to load signing key (${error.message})`,
            );
            return undefined;
          }),
        ),
      );
    }
    if (hasCompleteApnsConfig && !config.IOS_CONTROL_AUTH_TOKEN) {
      logger.warn(
        "iOS control APNs pushes disabled: IOS_CONTROL_AUTH_TOKEN is not set",
      );
    }
    return new IOSControlService(streamers, config.IOS_CONTROL_HOME_URL, logger, apns);
  });
}

// --run-task <name>: run a single task once and exit
const runTaskIndex = process.argv.indexOf("--run-task");
if (runTaskIndex !== -1) {
  const taskName = process.argv[runTaskIndex + 1];
  if (!taskName) {
    logger.error("Usage: --run-task <TaskName>");
    process.exit(1);
  }

  const loadedLiveCheck = loadStreamers();
  const oneOffStreamers = loadedLiveCheck.streamers;
  const oneOffControls = await runPromise(
    createIOSControlServiceEffect(oneOffStreamers),
  );
  const tasks = buildTasks(
    oneOffStreamers,
    oneOffControls,
    loadedLiveCheck.dggTopEmbeds,
    loadedLiveCheck.availablePlatforms,
  );
  const task = tasks.find((t) => t.name.toLowerCase() === taskName.toLowerCase());
  if (!task) {
    const names = tasks.map((t) => t.name).join(", ");
    logger.error(`Unknown task "${taskName}". Available: ${names}`);
    process.exit(1);
  }

  logger.info(`Running task "${task.name}" once...`);
  try {
    await task.run();
  } finally {
    if (livestreamIntelligence) {
      await runPromise(livestreamIntelligence.close());
    }
    oneOffControls.close();
  }
  logger.info(`Task "${task.name}" complete`);
  process.exit(0);
}

// --server-only: just the HTTP server, no tasks
const serverOnly = process.argv.includes("--server-only");

const registry = new TaskRegistry(logger);
const loadedLiveCheck = loadStreamers();
const streamers = loadedLiveCheck.streamers;
const iosControls = await runPromise(createIOSControlServiceEffect(streamers));
function requestWorkspaceEmailRun(
  workspaceId: string,
  subjectId: string,
  message: string,
  trigger: "email" = "email",
): EffectType<void, WorkspaceOperationError> {
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
    .runNowAndWaitEffect(definition.taskName, {
      message,
      subjectId,
      trigger,
    })
    .pipe(
      Effect.mapError(
        (cause) =>
          new WorkspaceOperationError({
            operation: "run email-triggered workspace",
            cause,
          }),
      ),
      Effect.tap(() =>
        Effect.sync(() =>
          logger.info(`Completed workspace email run for ${workspaceId}/${subjectId}`),
        ),
      ),
      Effect.asVoid,
    );
}

// Filled in once the email features start; powers the reprocess endpoint.
const emailControls: EmailControls = {};

// Start HTTP server
const closeServer = startServer(
  config.FRONTEND_PORT,
  logger,
  registry,
  streamers,
  emailControls,
  iosControls,
  livestreamIntelligence,
);

let cleanupEmailTransportEffect: EffectType<void, never> | undefined;
let emailStartupFiber: Fiber.Fiber<void, unknown> | undefined;

if (!serverOnly) {
  const scheduler = new Scheduler(logger);
  for (const task of buildTasks(
    streamers,
    iosControls,
    loadedLiveCheck.dggTopEmbeds,
    loadedLiveCheck.availablePlatforms,
  )) {
    scheduler.register(registry.track(task));
  }

  // Email tasks register up-front (Scheduler requires pre-start registration)
  // so a failed transport connect at boot can't silently disable them — the
  // exact outage the watchdog exists to catch. The retry task no-ops until
  // the controls fill in; the connect itself retries in the background.
  if (config.EMAIL_TRANSPORT) {
    scheduler.register(registry.track(new EmailWatchdogTask(logger)));
    scheduler.register(registry.track(new EmailRetryTask(() => emailControls, logger)));
    emailStartupFiber = Effect.runFork(startEmailWithRetryEffect(logger));
  } else {
    logger.info(
      "Email features disabled: no transport configured " +
        "(FASTMAIL_API_TOKEN or ICLOUD_USERNAME + ICLOUD_APP_PASSWORD)",
    );
  }

  // Start scheduler (runs opted-in tasks immediately, then all tasks on schedule)
  scheduler.start();
  const recoveryFiber = Effect.runFork(
    registry
      .recoverMissedTasksEffect()
      .pipe(
        Effect.catchCause((cause) =>
          Effect.sync(() => logger.error("Failed to recover missed task runs", cause)),
        ),
      ),
  );

  // Graceful shutdown handling
  let isShuttingDown = false;

  function shutdownEffect(signal: string): EffectType<void> {
    return Effect.suspend(() => {
      if (isShuttingDown) return Effect.void;
      isShuttingDown = true;
      logger.info(`Received ${signal}, shutting down gracefully...`);
      const safely = (name: string, effect: EffectType<unknown, unknown>) =>
        effect.pipe(
          Effect.catchCause((cause) =>
            Effect.sync(() => logger.error(`${name} shutdown failed`, cause)),
          ),
        );
      return Effect.all(
        [
          safely("HTTP server", fromPromise("close HTTP server", closeServer)),
          emailStartupFiber
            ? safely("email startup", Fiber.interrupt(emailStartupFiber))
            : Effect.void,
          safely("email transport", cleanupEmailTransportEffect ?? Effect.void),
          safely(
            "iOS controls",
            Effect.sync(() => iosControls.close()),
          ),
          safely(
            "scheduler",
            fromPromise("stop scheduler", () => scheduler.shutdown()),
          ),
          safely("manual task runs", registry.shutdownEffect()),
          livestreamIntelligence
            ? safely("livestream intelligence", livestreamIntelligence.close())
            : Effect.void,
          safely("task recovery", Fiber.interrupt(recoveryFiber)),
        ],
        { concurrency: "unbounded", discard: true },
      ).pipe(
        Effect.tap(() => Effect.sync(() => logger.info("Shutdown complete"))),
        Effect.tap(() => Effect.sync(() => process.exit(0))),
      );
    });
  }

  process.on("SIGTERM", () => void runPromise(shutdownEffect("SIGTERM")));
  process.on("SIGINT", () => void runPromise(shutdownEffect("SIGINT")));
} else {
  logger.info("Running in server-only mode (tasks disabled)");
}

interface EmailFeatures {
  cleanupEffect: EffectType<void, never>;
  transport: EmailTransport;
  handlers: Map<string, EmailHandler>;
}

/**
 * Containers restart often and mail servers can blip: a one-shot connect at
 * boot would silently disable the whole email system (including the retry
 * drain) until the next restart. Retry forever with capped backoff instead;
 * only the first failure alerts (errors reach Pushover), and the watchdog
 * covers the prolonged-outage case.
 */
function startEmailWithRetryEffect(parentLogger: Logger): EffectType<void, never> {
  let attempt = 0;
  return startEmailFeaturesEffect(parentLogger).pipe(
    Effect.tap((email) =>
      Effect.sync(() => {
        if (email) {
          cleanupEmailTransportEffect = email.cleanupEffect;
          emailControls.transport = email.transport;
          emailControls.handlers = email.handlers;
        }
      }),
    ),
    Effect.tapError((error) =>
      Effect.sync(() => {
        attempt += 1;
        const delaySeconds = Math.min(30 * 2 ** (attempt - 1), 300);
        const message = `Failed to start email features (attempt ${attempt}), retrying in ${delaySeconds}s`;
        if (attempt === 1) {
          parentLogger.error(message, error.message);
        } else {
          parentLogger.info(`${message}: ${error.message}`);
        }
      }),
    ),
    Effect.retry(
      Schedule.min([Schedule.exponential("30 seconds"), Schedule.spaced("5 minutes")]),
    ),
    Effect.asVoid,
    Effect.catch((error) =>
      Effect.sync(() => parentLogger.error("Email startup stopped", error)),
    ),
  );
}

function createEmailTransportEffect(
  logger: Logger,
): EffectType<
  EmailTransport | undefined,
  import("./effect/errors.js").IntegrationError
> {
  switch (config.EMAIL_TRANSPORT) {
    case "fastmail":
      if (!config.FASTMAIL_API_TOKEN) return Effect.succeed(undefined);
      return JmapTransport.createEffect(
        config.FASTMAIL_API_TOKEN,
        logger.extend("JMAP"),
      ).pipe(
        Effect.mapError(
          (cause) =>
            new IntegrationError({
              operation: "create JMAP transport",
              cause,
            }),
        ),
      );
    case "icloud":
      if (!config.ICLOUD_USERNAME || !config.ICLOUD_APP_PASSWORD) {
        return Effect.succeed(undefined);
      }
      return Effect.succeed(
        new ImapTransport(
          { user: config.ICLOUD_USERNAME, pass: config.ICLOUD_APP_PASSWORD },
          logger.extend("IMAP"),
        ),
      );
    default:
      return Effect.succeed(undefined);
  }
}

function startEmailFeaturesEffect(
  parentLogger: Logger,
): EffectType<
  EmailFeatures | undefined,
  import("./effect/errors.js").IntegrationError
> {
  return Effect.gen(function* () {
    const emailLogger = parentLogger.extend("Email");
    const transport = yield* createEmailTransportEffect(parentLogger);
    if (!transport) return undefined;

    // Create dispatcher and register handlers; one shared triage service so
    // concurrent pipelines classify each email with a single model call.
    const dispatcher = new EmailDispatcher(transport, emailLogger);
    const triage = new EmailTriageService(emailLogger.extend("Triage"));

    const parcel = createParcelHandler(parentLogger, triage);
    if (parcel) dispatcher.register(parcel);

    const calendar = createCalendarHandler(transport, parentLogger, triage);
    if (calendar) dispatcher.register(calendar);

    const workspaces = new WorkspaceEmailHandler(
      (workspaceId, subjectId, message, trigger) =>
        requestWorkspaceEmailRun(workspaceId, subjectId, message, trigger),
      emailLogger.extend("Workspaces"),
    );
    dispatcher.register(workspaces);

    if (dispatcher.handlerCount === 0) {
      emailLogger.info("No email pipelines active");
      return undefined;
    }

    const handlers = new Map<string, EmailHandler>();
    if (parcel) handlers.set(parcel.name, parcel);
    if (calendar) handlers.set(calendar.name, calendar);
    handlers.set(workspaces.name, workspaces);

    yield* dispatcher.startEffect.pipe(
      Effect.mapError(
        (cause) =>
          new IntegrationError({
            operation: "start email transport",
            cause,
          }),
      ),
    );
    emailLogger.info(
      `Started ${transport.name} transport with ${dispatcher.handlerCount} pipeline(s)`,
    );
    return { cleanupEffect: dispatcher.stopEffect, transport, handlers };
  });
}
