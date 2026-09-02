import type { Effect as EffectType } from "effect/Effect";
import { randomUUID } from "node:crypto";
import type { NamedLogger } from "@micthiesen/mitools/logging";
import type { Logger } from "@micthiesen/mitools/logging";
import type { Docstore } from "@micthiesen/mitools/docstore";
import type { Pushover } from "@micthiesen/mitools/pushover";
import { notify } from "@micthiesen/mitools/pushover";
import { Clock, Data, Effect, Fiber, Semaphore } from "effect";
import { recordCostEventSafely } from "../../costs/persistence.js";
import config from "../../utils/config.js";
import type { StreamerStatusLive } from "../persistence.js";
import { getNotificationUrlFields } from "../platforms/index.js";
import type { Streamer } from "../streamers.js";
import { alertSentInSession, livestreamAlertConfidenceFloor } from "./alertPolicy.js";
import { computeRelevance, ViewerAnomalyTracker } from "./anomaly.js";
import { LivestreamAudioCapture } from "./audio.js";
import {
  isTranscriptAlertType,
  LivestreamClassifier,
  livestreamSpendCents,
  type TranscriptAssessment,
  type TranscriptAssessmentInput,
} from "./classifier.js";
import { LocalSpeechRuntime, type SpeechRecognitionError } from "./localSpeech.js";
import {
  buildLivestreamFeedbackDigest,
  DESTINY_CONFIRMED_EVENT_TITLE,
  getLatestDestinyConfirmation,
  getLivestreamDiagnostics,
  getLivestreamIntelligence,
  recordLivestreamEvent,
  saveLivestreamIntelligence,
  updateLivestreamStage,
} from "./persistence.js";
import { decideVoiceMatchAction } from "./presencePolicy.js";
import { areSameLivestreamTopic } from "./summaryText.js";
import type {
  LivestreamAlertRecord,
  LivestreamAlertType,
  LivestreamEventKind,
  LivestreamIntelligenceData,
  LivestreamPipelineStage,
  LivestreamStageDiagnostic,
  RollingSummary,
} from "./types.js";
import { VoiceEvidenceTracker } from "./voiceEvidence.js";
import { selectVoiceTargets } from "./voiceTargets.js";

const DESTINY_ID = "destiny";
const PRESENCE_EXPIRY_MS = 10 * 60_000;
const ALERT_COOLDOWN_MS = 30 * 60_000;
const MAX_CHAPTERS = 40;
const TRANSCRIPT_EXCERPT_CHARS = 800;

function failureMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error !== null && "message" in error) {
    return String(error.message);
  }
  if (typeof error === "object" && error !== null && "cause" in error) {
    return failureMessage(error.cause);
  }
  return String(error);
}

export class LivestreamNotificationError extends Data.TaggedError(
  "LivestreamNotificationError",
)<{ readonly cause: unknown }> {}

export function isDestinyOwnedStream(streamer: Streamer): boolean {
  const identifiers = [
    streamer.id,
    streamer.displayName,
    ...streamer.bindings.map((binding) => binding.username),
  ];
  return identifiers.some(
    (value) => value.trim().toLowerCase().replace(/^@/, "") === DESTINY_ID,
  );
}

export interface LiveObservation {
  streamer: Streamer;
  status: StreamerStatusLive;
  wentLive: boolean;
  titleChanged: boolean;
}

export function viewerCountForAnomaly(status: StreamerStatusLive): number | null {
  if (!status.sources) return status.viewerCount ?? null;
  const primarySource = status.sources.find(
    (source) =>
      source.platform === status.primary.platform &&
      source.username === status.primary.username,
  );
  return primarySource?.viewerCount ?? null;
}

export interface LivestreamIntelligenceObserver {
  observeLive(
    observation: LiveObservation,
  ): EffectType<void, unknown, Logger | Docstore | Pushover>;
  observeOffline(
    streamerId: string,
  ): EffectType<void, unknown, Logger | Docstore | Pushover>;
  afterTick(): EffectType<void, unknown, Logger | Docstore | Pushover>;
  close(): EffectType<void>;
}

export interface LivestreamRuntimeDiagnostics {
  enabled: true;
  voiceprintLoaded: boolean;
  model: string;
  queues: {
    capture: { running: number; queued: number };
    speech: { running: number; queued: number };
    llm: { running: number; queued: number };
  };
  activeStreamCount: number;
  activeVoiceTargetCount: number;
  budget: { spentCents: number; limitCents: number; remainingCents: number };
  intervals: { voiceSeconds: number; summarySeconds: number };
}

export interface LivestreamIntelligenceDiagnosticsProvider {
  getRuntimeDiagnostics(): EffectType<LivestreamRuntimeDiagnostics, unknown, Docstore>;
}

export interface LivestreamIntelligenceDependencies {
  readonly capture?: Pick<LivestreamAudioCapture, "captureEffect">;
  readonly speech: Pick<
    LocalSpeechRuntime,
    "hasVoiceprint" | "detectDestinyEffect" | "transcribeEffect"
  >;
  readonly classifier?: Pick<LivestreamClassifier, "assessTranscriptEffect">;
}

function epoch(value: Date | string): number {
  return new Date(value).getTime();
}

function streamUrl(status: StreamerStatusLive): string {
  return (
    status.primary.urlOverride ??
    getNotificationUrlFields(status.primary.platform, status.primary.username).url
  );
}

function newState(
  observation: LiveObservation,
  now: number,
): LivestreamIntelligenceData {
  return {
    streamerId: observation.streamer.id,
    sessionStartedAt: epoch(observation.status.startedAt),
    relevanceScore: observation.streamer.tier === "primary" ? 40 : 15,
    relevanceReasons:
      observation.streamer.tier === "primary" ? ["primary channel"] : [],
    chapters: [],
    updatedAt: now,
  };
}

function concatSamples(...parts: Float32Array[]): Float32Array {
  const total = parts.reduce((sum, part) => sum + part.length, 0);
  const joined = new Float32Array(total);
  let offset = 0;
  for (const part of parts) {
    joined.set(part, offset);
    offset += part.length;
  }
  return joined;
}

/** Compatibility edge for callback-oriented classifier/audio APIs. Scheduling
 * and concurrency are owned by Effect's semaphore, not an ambient promise queue. */
export class EffectWorkQueueClosedError extends Data.TaggedError(
  "EffectWorkQueueClosedError",
)<{}> {}

export class EffectWorkQueue {
  private readonly semaphore: Semaphore.Semaphore;
  private readonly fibers = new Set<Fiber.Fiber<unknown, unknown>>();
  private readonly drainWaiters = new Set<(effect: EffectType<void>) => void>();
  private accepting = true;
  private outstanding = 0;
  public pending = 0;
  public size = 0;

  public constructor(concurrency: number) {
    this.semaphore = Semaphore.makeUnsafe(concurrency);
  }

  private admit<A, E>(
    job: EffectType<A, E>,
  ): EffectType<EffectType<A, E>, EffectWorkQueueClosedError> {
    return Effect.suspend(() => {
      if (!this.accepting) return Effect.fail(new EffectWorkQueueClosedError());
      this.outstanding += 1;
      this.size += 1;
      let started = false;
      return Effect.succeed(
        this.semaphore
          .withPermits(1)(
            Effect.acquireUseRelease(
              Effect.sync(() => {
                started = true;
                this.size -= 1;
                this.pending += 1;
              }),
              () => job,
              () =>
                Effect.sync(() => {
                  this.pending -= 1;
                }),
            ),
          )
          .pipe(
            Effect.ensuring(
              Effect.sync(() => {
                if (!started) this.size -= 1;
                this.outstanding -= 1;
                if (this.outstanding === 0) {
                  for (const resume of this.drainWaiters) resume(Effect.void);
                  this.drainWaiters.clear();
                }
              }),
            ),
          ),
      );
    });
  }

  public run<A, E, R>(job: EffectType<A, E, R>) {
    return Effect.uninterruptibleMask((restore) =>
      Effect.gen({ self: this }, function* () {
        const context = yield* Effect.context<R>();
        const admitted = yield* this.admit(Effect.provide(job, context));
        return yield* restore(admitted);
      }),
    );
  }

  public fork<E, R>(job: EffectType<unknown, E, R>) {
    return Effect.uninterruptibleMask(() =>
      Effect.gen({ self: this }, function* () {
        const context = yield* Effect.context<R>();
        const admitted = yield* this.admit(Effect.provide(job, context));
        const fiber = yield* Effect.forkDetach(Effect.interruptible(admitted));
        this.fibers.add(fiber);
        fiber.addObserver(() => this.fibers.delete(fiber));
      }),
    ).pipe(Effect.catchTag("EffectWorkQueueClosedError", () => Effect.void));
  }

  public onIdle(): EffectType<void> {
    return Effect.suspend(() => {
      if (this.outstanding === 0) return Effect.void;
      return Effect.callback<void>((resume) => {
        this.drainWaiters.add(resume);
        if (this.outstanding === 0) {
          this.drainWaiters.delete(resume);
          resume(Effect.void);
        }
        return Effect.sync(() => this.drainWaiters.delete(resume));
      });
    });
  }

  public close(): EffectType<void> {
    return Effect.gen({ self: this }, function* () {
      this.accepting = false;
      const drained = yield* this.onIdle().pipe(
        Effect.timeout("30 seconds"),
        Effect.result,
      );
      if (drained._tag === "Failure") {
        yield* Fiber.interruptAll([...this.fibers]);
        yield* this.onIdle();
      }
    });
  }
}

export class LivestreamIntelligenceService implements LivestreamIntelligenceObserver {
  private readonly anomaly = new ViewerAnomalyTracker();
  private readonly capture: Pick<LivestreamAudioCapture, "captureEffect">;
  private readonly speech: Pick<
    LocalSpeechRuntime,
    "hasVoiceprint" | "detectDestinyEffect" | "transcribeEffect"
  >;
  private readonly classifier: Pick<LivestreamClassifier, "assessTranscriptEffect">;
  private readonly captureQueue = new EffectWorkQueue(2);
  private readonly speechQueue = new EffectWorkQueue(1);
  private readonly llmQueue = new EffectWorkQueue(1);
  private readonly backgroundQueue = new EffectWorkQueue(8);
  private readonly pendingVoice = new Map<string, number>();
  private readonly pendingSummary = new Map<string, number>();
  private readonly lastVoiceSample = new Map<string, number>();
  private readonly lastSummary = new Map<string, number>();
  private readonly voiceEvidence = new VoiceEvidenceTracker();
  private readonly active = new Map<string, LiveObservation>();
  private readonly alertTimes = new Map<string, number>();
  private voiceprintWarningLogged = false;

  public constructor(
    private readonly logger: NamedLogger,
    dependencies: LivestreamIntelligenceDependencies,
  ) {
    this.capture = dependencies.capture ?? new LivestreamAudioCapture();
    this.classifier = dependencies.classifier ?? new LivestreamClassifier(logger);
    this.speech = dependencies.speech;
  }

  public getRuntimeDiagnostics() {
    return Effect.gen({ self: this }, function* () {
      const spentCents = yield* livestreamSpendCents();
      const limitCents = config.LIVESTREAM_MONTHLY_BUDGET_USD * 100;
      return {
        enabled: true as const,
        voiceprintLoaded: this.speech.hasVoiceprint,
        model: "parakeet-tdt-0.6b-v3-int8",
        queues: {
          capture: {
            running: this.captureQueue.pending,
            queued: this.captureQueue.size,
          },
          speech: { running: this.speechQueue.pending, queued: this.speechQueue.size },
          llm: { running: this.llmQueue.pending, queued: this.llmQueue.size },
        },
        activeStreamCount: this.active.size,
        activeVoiceTargetCount: [...this.active.values()].filter((item) =>
          this.isVoiceTarget(item),
        ).length,
        budget: {
          spentCents,
          limitCents,
          remainingCents: Math.max(0, limitCents - spentCents),
        },
        intervals: {
          voiceSeconds: config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS,
          summarySeconds: config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS,
        },
      };
    });
  }

  private recordEvent(input: {
    streamerId: string;
    sessionStartedAt?: number;
    kind: LivestreamEventKind;
    status: "info" | "success" | "warning" | "error";
    title: string;
    detail?: string;
    durationMs?: number;
    costCents?: number;
    metrics?: Record<string, number | string | boolean | null>;
  }) {
    return recordLivestreamEvent(input).pipe(
      Effect.catch((error) =>
        this.logger.warn(
          `Failed to persist intelligence event: ${failureMessage(error)}`,
        ),
      ),
      Effect.asVoid,
    );
  }

  private updateStage(
    streamerId: string,
    sessionStartedAt: number,
    stage: LivestreamPipelineStage,
    value: LivestreamStageDiagnostic,
  ) {
    return updateLivestreamStage(streamerId, sessionStartedAt, stage, value).pipe(
      Effect.catch((error) =>
        this.logger.warn(
          `Failed to persist ${stage} diagnostics: ${failureMessage(error)}`,
        ),
      ),
      Effect.asVoid,
    );
  }

  public observeLive(observation: LiveObservation) {
    return Effect.gen({ self: this }, function* () {
      if (!this.speech.hasVoiceprint && !this.voiceprintWarningLogged) {
        yield* this.logger.warn(
          "Livestream intelligence started without a Destiny voiceprint; summaries are enabled but guest detection is disabled",
        );
        this.voiceprintWarningLogged = true;
      }
      const now = yield* Clock.currentTimeMillis;
      this.active.set(observation.streamer.id, observation);
      const previous = yield* getLivestreamIntelligence(observation.streamer.id);
      const sessionStartedAt = epoch(observation.status.startedAt);
      const isNewSession = previous?.sessionStartedAt !== sessionStartedAt;
      if (isNewSession) this.anomaly.clear(observation.streamer.id);
      let state =
        previous?.sessionStartedAt === sessionStartedAt
          ? previous
          : newState(observation, now);
      const trend = this.anomaly.observe({
        streamerId: observation.streamer.id,
        viewers: viewerCountForAnomaly(observation.status),
        dggViewers: observation.streamer.dgg?.viewers ?? null,
        sessionStartedAt,
        now,
      });
      const presenceFresh =
        state.destinyPresence &&
        now - state.destinyPresence.detectedAt <= PRESENCE_EXPIRY_MS;
      if (state.destinyPresence && !presenceFresh) {
        state = { ...state, destinyPresence: undefined };
      }
      // Title-only model classification mostly paraphrased the visible title.
      // Keep intelligence grounded in measured audience data and captured audio.
      state = { ...state, semantic: undefined };
      const relevance = computeRelevance({
        streamer: observation.streamer,
        trend,
        destinyConfirmed: state.destinyPresence?.state === "confirmed",
      });
      state = {
        ...state,
        trend,
        relevanceScore: relevance.score,
        relevanceReasons: relevance.reasons,
        updatedAt: now,
      };
      yield* saveLivestreamIntelligence(state);

      const previousDiagnostics = yield* getLivestreamDiagnostics(
        observation.streamer.id,
      );
      if (isNewSession) {
        yield* this.recordEvent({
          streamerId: observation.streamer.id,
          sessionStartedAt,
          kind: "session",
          status: "info",
          title: "Live session started",
          detail: observation.status.primaryTitle,
        });
      }
      if (isNewSession || previousDiagnostics?.sessionStartedAt !== sessionStartedAt) {
        yield* this.updateStage(observation.streamer.id, sessionStartedAt, "metadata", {
          status: "skipped",
          eligible: false,
          finishedAt: now,
          detail: "Title-only LLM classification is disabled",
        });
        yield* this.updateStage(observation.streamer.id, sessionStartedAt, "voice", {
          status: "idle",
          eligible: this.isVoiceTarget(observation),
          detail: this.isVoiceTarget(observation)
            ? "Eligible DGG third-party stream"
            : isDestinyOwnedStream(observation.streamer)
              ? "Destiny's own stream is always excluded"
              : "Not a DGG third-party voice target",
        });
        yield* this.updateStage(observation.streamer.id, sessionStartedAt, "summary", {
          status: state.summary ? "success" : "idle",
          eligible: this.isSummaryTarget(observation, state),
          finishedAt: state.summary?.updatedAt,
          detail:
            state.summary?.topic ??
            (this.isSummaryTarget(observation, state)
              ? "Eligible for rolling summaries"
              : "Waiting for relevance, importance, or anomaly gate"),
          metrics: state.summary
            ? {
                confidence: state.summary.confidence,
                audioSeconds: state.summary.windowSeconds,
              }
            : undefined,
        });
        if (state.latestAlert) {
          yield* this.updateStage(observation.streamer.id, sessionStartedAt, "alert", {
            status: "success",
            eligible: true,
            finishedAt: state.latestAlert.createdAt,
            detail: state.latestAlert.title,
            metrics: {
              type: state.latestAlert.type,
              confidence: state.latestAlert.confidence,
            },
          });
        }
      }

      if (trend.anomalous && relevance.score >= 70) {
        if (!previous?.trend?.anomalous) {
          yield* this.recordEvent({
            streamerId: observation.streamer.id,
            sessionStartedAt,
            kind: "anomaly",
            status: "warning",
            title: "Viewer surge detected",
            detail: trend.reason ?? undefined,
            metrics: {
              percentChange: trend.percentChange,
              viewersPerMinute: trend.viewersPerMinute,
              dggPercentChange: trend.dggPercentChange,
            },
          });
        }
        yield* this.backgroundQueue.fork(
          this.maybeNotify({
            observation,
            type: "viewer_surge",
            title: `${observation.streamer.displayName} is surging`,
            message: trend.reason ?? "Viewer activity rose unusually quickly.",
            reason: trend.reason ?? "Unusual viewer acceleration",
            confidence: 0.85,
          }).pipe(
            Effect.catch((error) =>
              this.logger.warn(
                `Viewer surge notification failed for ${observation.streamer.displayName}: ${failureMessage(error)}`,
              ),
            ),
          ),
        );
      }

      if (
        this.isSummaryTarget(observation, state) &&
        this.summaryDue(observation.streamer.id, now)
      ) {
        yield* this.scheduleSummary(observation);
      }
    });
  }

  public observeOffline(streamerId: string) {
    return Effect.gen({ self: this }, function* () {
      const current = yield* getLivestreamIntelligence(streamerId);
      const diagnostics = yield* getLivestreamDiagnostics(streamerId);
      const finishedAt = yield* Clock.currentTimeMillis;
      if (this.active.has(streamerId) && current) {
        yield* this.recordEvent({
          streamerId,
          sessionStartedAt: current.sessionStartedAt,
          kind: "session",
          status: "info",
          title: "Live session ended",
        });
        for (const [stage, value] of Object.entries(diagnostics?.stages ?? {})) {
          if (value?.status !== "running") continue;
          yield* this.updateStage(
            streamerId,
            current.sessionStartedAt,
            stage as LivestreamPipelineStage,
            {
              ...value,
              status: "skipped",
              finishedAt,
              durationMs: value.startedAt ? finishedAt - value.startedAt : undefined,
              detail: "Stream ended before this operation completed",
            },
          );
        }
      }
      this.active.delete(streamerId);
      this.anomaly.clear(streamerId);
      this.voiceEvidence.clear(streamerId);
      this.pendingVoice.delete(streamerId);
      this.pendingSummary.delete(streamerId);
      this.lastVoiceSample.delete(streamerId);
      this.lastSummary.delete(streamerId);
    });
  }

  public afterTick() {
    return Effect.gen({ self: this }, function* () {
      const now = yield* Clock.currentTimeMillis;
      const targets = selectVoiceTargets(
        [...this.active.values()].filter((observation) =>
          this.isVoiceTarget(observation),
        ),
        config.LIVESTREAM_MAX_VOICE_TARGETS,
      );
      for (const observation of targets) {
        if (this.voiceDue(observation.streamer.id, now)) {
          yield* this.scheduleVoiceSample(observation);
        }
      }
    });
  }

  public close(): EffectType<void> {
    // Drain in dependency order: capture jobs may still enqueue speech work,
    // and speech work may still enqueue classifier work.
    return this.captureQueue
      .close()
      .pipe(
        Effect.andThen(this.speechQueue.close()),
        Effect.andThen(this.llmQueue.close()),
        Effect.andThen(this.backgroundQueue.close()),
      );
  }

  private captureEffect(url: string, seconds: number) {
    return this.capture.captureEffect(url, seconds);
  }

  private transcribeEffect(samples: Float32Array) {
    return this.speech.transcribeEffect(samples);
  }

  private assessTranscriptEffect(input: TranscriptAssessmentInput) {
    return this.classifier.assessTranscriptEffect(input);
  }

  private isVoiceTarget(observation: LiveObservation): boolean {
    return (
      this.speech.hasVoiceprint &&
      !isDestinyOwnedStream(observation.streamer) &&
      observation.streamer.dgg !== undefined
    );
  }

  private currentSession(observation: LiveObservation) {
    if (!this.active.has(observation.streamer.id)) return Effect.succeed(undefined);
    return getLivestreamIntelligence(observation.streamer.id).pipe(
      Effect.map((current) =>
        current?.sessionStartedAt === epoch(observation.status.startedAt)
          ? current
          : undefined,
      ),
    );
  }

  private isSummaryTarget(
    observation: LiveObservation,
    state: LivestreamIntelligenceData,
  ): boolean {
    return (
      observation.streamer.tier === "primary" ||
      state.destinyPresence?.state === "confirmed" ||
      state.trend?.anomalous === true ||
      state.relevanceScore >= 80
    );
  }

  private voiceDue(streamerId: string, now: number): boolean {
    return (
      !this.pendingVoice.has(streamerId) &&
      now - (this.lastVoiceSample.get(streamerId) ?? 0) >=
        config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000
    );
  }

  private summaryDue(streamerId: string, now: number): boolean {
    return (
      !this.pendingSummary.has(streamerId) &&
      now - (this.lastSummary.get(streamerId) ?? 0) >=
        config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000
    );
  }

  private scheduleVoiceSample(observation: LiveObservation) {
    return Effect.gen({ self: this }, function* () {
      const id = observation.streamer.id;
      const sessionStartedAt = epoch(observation.status.startedAt);
      const startedAt = yield* Clock.currentTimeMillis;
      this.pendingVoice.set(id, sessionStartedAt);
      this.lastVoiceSample.set(id, startedAt);
      yield* this.updateStage(id, sessionStartedAt, "voice", {
        status: "running",
        eligible: true,
        startedAt,
        detail: "Capturing a bounded voice sample",
      });
      const job = Effect.gen({ self: this }, function* () {
        const audio = yield* this.captureEffect(
          streamUrl(observation.status),
          config.LIVESTREAM_VOICE_SAMPLE_SECONDS,
        );
        yield* this.speechQueue.run(
          Effect.gen({ self: this }, function* () {
            const match = yield* this.speech.detectDestinyEffect(audio.samples);
            yield* this.handleVoiceMatch(observation, audio.samples, match, startedAt);
          }),
        );
      }).pipe(
        Effect.catch((error) =>
          Effect.gen({ self: this }, function* () {
            const finishedAt = yield* Clock.currentTimeMillis;
            const detail = failureMessage(error).slice(0, 500);
            yield* this.updateStage(id, sessionStartedAt, "voice", {
              status: "error",
              eligible: true,
              startedAt,
              finishedAt,
              nextAt:
                startedAt + config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000,
              durationMs: finishedAt - startedAt,
              detail,
            });
            yield* this.recordEvent({
              streamerId: id,
              sessionStartedAt,
              kind: "voice",
              status: "error",
              title: "Voice scan failed",
              detail,
              durationMs: finishedAt - startedAt,
            });
            yield* this.logger.warn(
              `Voice sampling failed for ${observation.streamer.displayName}: ${detail}`,
            );
          }),
        ),
        Effect.ensuring(
          Effect.sync(() => {
            if (this.pendingVoice.get(id) === sessionStartedAt) {
              this.pendingVoice.delete(id);
            }
          }),
        ),
      );
      return yield* this.captureQueue.fork(job);
    });
  }

  private handleVoiceMatch(
    observation: LiveObservation,
    samples: Float32Array,
    match: { confidence: number; matchedWindows: number; checkedWindows: number },
    startedAt: number,
  ) {
    return Effect.gen({ self: this }, function* () {
      const id = observation.streamer.id;
      const now = yield* Clock.currentTimeMillis;
      const current = yield* this.currentSession(observation);
      if (!current) return;
      const evidence = this.voiceEvidence.observe(
        id,
        match.matchedWindows,
        match.checkedWindows,
        now,
      );
      const priorConfirmation = yield* getLatestDestinyConfirmation(
        id,
        current.sessionStartedAt,
      );
      const confirmedPresence =
        current.destinyPresence?.state === "confirmed"
          ? current.destinyPresence
          : priorConfirmation
            ? {
                state: "confirmed" as const,
                confidence: Math.min(
                  Number(priorConfirmation.metrics?.speakerConfidence ?? 0),
                  Number(priorConfirmation.metrics?.assessmentConfidence ?? 0),
                ),
                detectedAt: priorConfirmation.createdAt,
                reason:
                  priorConfirmation.detail ?? "Live conversation previously confirmed",
              }
            : current.destinyPresence;
      const action = decideVoiceMatchAction(evidence, confirmedPresence);
      const voiceDetail = `${match.matchedWindows}/${match.checkedWindows} windows matched at ${Math.round(match.confidence * 100)}% confidence`;
      yield* this.updateStage(id, current.sessionStartedAt, "voice", {
        status: "success",
        eligible: true,
        startedAt,
        finishedAt: now,
        nextAt: startedAt + config.LIVESTREAM_VOICE_SAMPLE_INTERVAL_SECONDS * 1000,
        durationMs: now - startedAt,
        detail:
          evidence === "none" ? `No Destiny evidence; ${voiceDetail}` : voiceDetail,
        metrics: {
          confidence: match.confidence,
          matchedWindows: match.matchedWindows,
          checkedWindows: match.checkedWindows,
          evidence: action === "retain_confirmed" ? "confirmed" : evidence,
        },
      });
      if (action === "ignore") return;
      if (action === "retain_confirmed" && confirmedPresence) {
        const presence = {
          ...confirmedPresence,
          detectedAt: now,
        };
        const latest = yield* this.currentSession(observation);
        if (!latest) return;
        yield* saveLivestreamIntelligence({
          ...latest,
          destinyPresence: presence,
          updatedAt: now,
        });
        if (!alertSentInSession(latest, "destiny_guest")) {
          yield* this.maybeNotify({
            observation,
            type: "destiny_guest",
            title: `Destiny is on ${observation.streamer.displayName}`,
            message: presence.reason,
            reason: "Previously confirmed live participation plus fresh voice evidence",
            confidence: presence.confidence,
          });
        }
        return;
      }
      if (action === "record_possible") {
        const latest = yield* this.currentSession(observation);
        if (!latest) return;
        yield* saveLivestreamIntelligence({
          ...latest,
          destinyPresence: {
            state: "possible",
            confidence: match.confidence,
            detectedAt: now,
            reason: `${match.matchedWindows}/${match.checkedWindows} voice windows matched; awaiting confirmation`,
          },
          updatedAt: now,
        });
        yield* this.recordEvent({
          streamerId: id,
          sessionStartedAt: current.sessionStartedAt,
          kind: "voice",
          status: "warning",
          title: "Possible Destiny voice match",
          detail: `${voiceDetail}; awaiting another independent sample`,
          durationMs: now - startedAt,
          metrics: {
            confidence: match.confidence,
            matchedWindows: match.matchedWindows,
            checkedWindows: match.checkedWindows,
          },
        });
        return;
      }

      const context = yield* this.captureEffect(streamUrl(observation.status), 45);
      const combined = concatSamples(samples, context.samples);
      const transcript = yield* this.transcribeEffect(combined);
      yield* this.recordLocalTranscription("verify-destiny", combined.length / 16_000);
      if (transcript.length < 30) {
        yield* this.recordEvent({
          streamerId: id,
          sessionStartedAt: current.sessionStartedAt,
          kind: "voice",
          status: "warning",
          title: "Voice confirmation skipped",
          detail: "The confirmation window did not contain enough transcribed speech",
        });
        return;
      }
      const assessment = yield* this.llmQueue.run(
        this.assessTranscriptEffect({
          displayName: observation.streamer.displayName,
          title: observation.status.primaryTitle,
          transcript,
          previousSummary: current.summary?.text,
          previousTopic: current.summary?.topic,
          viewerAnomaly: current.trend?.reason,
          speakerMatchConfidence: match.confidence,
          testingDestinyPresence: true,
        }),
      );
      if (!assessment?.destinyIsLiveParticipant || assessment.confidence < 0.65) {
        yield* this.recordEvent({
          streamerId: id,
          sessionStartedAt: current.sessionStartedAt,
          kind: "voice",
          status: "info",
          title: "Destiny live participation not confirmed",
          detail:
            assessment?.summary ?? "Transcript assessment skipped by the budget gate",
          metrics: { assessmentConfidence: assessment?.confidence ?? null },
        });
        return;
      }
      const latest = yield* this.currentSession(observation);
      if (!latest) return;
      const confirmedAt = yield* Clock.currentTimeMillis;
      yield* saveLivestreamIntelligence({
        ...latest,
        destinyPresence: {
          state: "confirmed",
          confidence: Math.min(match.confidence, assessment.confidence),
          detectedAt: confirmedAt,
          reason: assessment.summary,
        },
        updatedAt: confirmedAt,
      });
      yield* this.recordEvent({
        streamerId: id,
        sessionStartedAt: latest.sessionStartedAt,
        kind: "voice",
        status: "success",
        title: DESTINY_CONFIRMED_EVENT_TITLE,
        detail: assessment.summary,
        metrics: {
          speakerConfidence: match.confidence,
          assessmentConfidence: assessment.confidence,
        },
      });
      yield* this.maybeNotify({
        observation,
        type: "destiny_guest",
        title: `Destiny is on ${observation.streamer.displayName}`,
        message: assessment.summary,
        reason: `${match.matchedWindows} repeated voice matches plus live-conversation context`,
        confidence: Math.min(match.confidence, assessment.confidence),
      });
    });
  }

  private scheduleSummary(observation: LiveObservation) {
    return Effect.gen({ self: this }, function* () {
      const id = observation.streamer.id;
      const sessionStartedAt = epoch(observation.status.startedAt);
      const startedAt = yield* Clock.currentTimeMillis;
      const costBefore = yield* livestreamSpendCents(startedAt);
      this.pendingSummary.set(id, sessionStartedAt);
      this.lastSummary.set(id, startedAt);
      yield* this.updateStage(id, sessionStartedAt, "summary", {
        status: "running",
        eligible: true,
        startedAt,
        detail: "Capturing and transcribing the current window",
      });
      const job = Effect.gen({ self: this }, function* () {
        const audio = yield* this.captureEffect(
          streamUrl(observation.status),
          config.LIVESTREAM_SUMMARY_SAMPLE_SECONDS,
        );
        const transcript = yield* this.speechQueue.run(
          this.transcribeEffect(audio.samples),
        );
        yield* this.recordLocalTranscription("rolling-summary", audio.durationSeconds);
        if (!transcript || transcript.length < 30) {
          const finishedAt = yield* Clock.currentTimeMillis;
          yield* this.updateStage(id, sessionStartedAt, "summary", {
            status: "skipped",
            eligible: true,
            startedAt,
            finishedAt,
            nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
            durationMs: finishedAt - startedAt,
            detail: "Not enough speech in the captured window",
          });
          yield* this.recordEvent({
            streamerId: id,
            sessionStartedAt,
            kind: "summary",
            status: "info",
            title: "Summary window skipped",
            detail: "Not enough transcribed speech",
            durationMs: finishedAt - startedAt,
          });
          return;
        }
        const current = yield* this.currentSession(observation);
        if (!current) return;
        const assessment = yield* this.llmQueue.run(
          this.assessTranscriptEffect({
            displayName: observation.streamer.displayName,
            title: observation.status.primaryTitle,
            transcript,
            previousSummary: current.summary?.text,
            previousTopic: current.summary?.topic,
            viewerAnomaly: current.trend?.reason,
            testingDestinyPresence: false,
          }),
        );
        if (!assessment) {
          const finishedAt = yield* Clock.currentTimeMillis;
          yield* this.updateStage(id, sessionStartedAt, "summary", {
            status: "skipped",
            eligible: true,
            startedAt,
            finishedAt,
            nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
            durationMs: finishedAt - startedAt,
            detail: "Monthly budget could not cover transcript assessment",
          });
          yield* this.recordEvent({
            streamerId: id,
            sessionStartedAt,
            kind: "summary",
            status: "warning",
            title: "Summary assessment skipped",
            detail: "Monthly intelligence budget gate",
          });
          return;
        }
        yield* this.saveAssessment(
          observation,
          transcript,
          audio.durationSeconds,
          assessment,
        );
        const finishedAt = yield* Clock.currentTimeMillis;
        const costCents = Math.max(
          0,
          (yield* livestreamSpendCents(finishedAt)) - costBefore,
        );
        yield* this.updateStage(id, sessionStartedAt, "summary", {
          status: "success",
          eligible: true,
          startedAt,
          finishedAt,
          nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
          durationMs: finishedAt - startedAt,
          detail: assessment.topic,
          metrics: {
            confidence: assessment.confidence,
            audioSeconds: Math.round(audio.durationSeconds),
            transcriptCharacters: transcript.length,
            costCents,
          },
        });
        yield* this.recordEvent({
          streamerId: id,
          sessionStartedAt,
          kind: "summary",
          status: "success",
          title: "Now summary updated",
          detail: assessment.topic,
          durationMs: finishedAt - startedAt,
          costCents,
          metrics: {
            confidence: assessment.confidence,
            audioSeconds: Math.round(audio.durationSeconds),
          },
        });
      }).pipe(
        Effect.catch((error) =>
          Effect.gen({ self: this }, function* () {
            const finishedAt = yield* Clock.currentTimeMillis;
            const detail = failureMessage(error).slice(0, 500);
            yield* this.updateStage(id, sessionStartedAt, "summary", {
              status: "error",
              eligible: true,
              startedAt,
              finishedAt,
              nextAt: startedAt + config.LIVESTREAM_SUMMARY_INTERVAL_SECONDS * 1000,
              durationMs: finishedAt - startedAt,
              detail,
            });
            yield* this.recordEvent({
              streamerId: id,
              sessionStartedAt,
              kind: "summary",
              status: "error",
              title: "Rolling summary failed",
              detail,
              durationMs: finishedAt - startedAt,
            });
            yield* this.logger.warn(
              `Rolling summary failed for ${observation.streamer.displayName}: ${detail}`,
            );
          }),
        ),
        Effect.ensuring(
          Effect.sync(() => {
            if (this.pendingSummary.get(id) === sessionStartedAt) {
              this.pendingSummary.delete(id);
            }
          }),
        ),
      );
      return yield* this.captureQueue.fork(job);
    });
  }

  private saveAssessment(
    observation: LiveObservation,
    transcript: string,
    durationSeconds: number,
    assessment: TranscriptAssessment,
  ) {
    return Effect.gen({ self: this }, function* () {
      const current = yield* this.currentSession(observation);
      if (!current) return;
      const now = yield* Clock.currentTimeMillis;
      const summary: RollingSummary = {
        text: assessment.summary,
        topic: assessment.topic,
        confidence: assessment.confidence,
        transcriptExcerpt: transcript.slice(-TRANSCRIPT_EXCERPT_CHARS),
        updatedAt: now,
        windowSeconds: Math.round(durationSeconds),
      };
      const lastChapter = current.chapters.at(-1);
      const chapters =
        lastChapter && areSameLivestreamTopic(lastChapter.title, assessment.topic)
          ? [
              ...current.chapters.slice(0, -1),
              { ...lastChapter, summary: assessment.summary },
            ]
          : [
              ...current.chapters,
              {
                chapterId: randomUUID(),
                startedAt: now - durationSeconds * 1000,
                title: assessment.topic,
                summary: assessment.summary,
              },
            ].slice(-MAX_CHAPTERS);
      yield* saveLivestreamIntelligence({
        ...current,
        summary,
        chapters,
        updatedAt: now,
      });
      if (
        isTranscriptAlertType(assessment.alertType) &&
        assessment.alertReason &&
        assessment.importance >= 80 &&
        assessment.confidence >= 0.75
      ) {
        yield* this.maybeNotify({
          observation,
          type: assessment.alertType,
          title: `${observation.streamer.displayName}: ${assessment.topic}`,
          message: assessment.summary,
          reason: assessment.alertReason,
          confidence: assessment.confidence,
        }).pipe(
          Effect.catch((error) =>
            this.logger.warn(
              `Semantic alert failed for ${observation.streamer.displayName}: ${failureMessage(error)}`,
            ),
          ),
        );
      }
    });
  }

  private recordLocalTranscription(operation: string, seconds: number) {
    return recordCostEventSafely({
      category: "transcription",
      feature: "livestream-intelligence",
      operation,
      service: "self-hosted",
      model: "parakeet-tdt-0.6b-v3-int8",
      costCents: 0,
      priceStatus: "free",
      usage: { requests: 1, characters: Math.round(seconds) },
    });
  }

  private feedbackConfidenceFloor(type: LivestreamAlertType) {
    return buildLivestreamFeedbackDigest(100).pipe(
      Effect.map((feedbackDigest) =>
        livestreamAlertConfidenceFloor({
          type,
          feedbackDigest,
          destinySpeakerThreshold: config.LIVESTREAM_DESTINY_SPEAKER_THRESHOLD,
        }),
      ),
    );
  }

  private maybeNotify(input: {
    observation: LiveObservation;
    type: LivestreamAlertType;
    title: string;
    message: string;
    reason: string;
    confidence: number;
  }) {
    return Effect.gen({ self: this }, function* () {
      const current = yield* this.currentSession(input.observation);
      if (!current) return;
      const now = yield* Clock.currentTimeMillis;
      const markSkipped = (detail: string) =>
        this.updateStage(
          input.observation.streamer.id,
          current.sessionStartedAt,
          "alert",
          {
            status: "skipped",
            eligible: true,
            finishedAt: now,
            detail,
            metrics: { type: input.type, confidence: input.confidence },
          },
        );
      const confidenceFloor = yield* this.feedbackConfidenceFloor(input.type);
      if (input.confidence < confidenceFloor) {
        yield* markSkipped(
          `${Math.round(input.confidence * 100)}% confidence was below the ${Math.round(confidenceFloor * 100)}% alert threshold`,
        );
        return;
      }
      const key = `${input.observation.streamer.id}:${current.sessionStartedAt}:${input.type}`;
      const previous = this.alertTimes.get(key) ?? 0;
      if (now - previous < ALERT_COOLDOWN_MS) {
        yield* markSkipped(
          "A matching alert was already sent within the 30-minute cooldown",
        );
        return;
      }
      if (
        (input.type === "destiny_guest" || input.type === "viewer_surge") &&
        alertSentInSession(current, input.type)
      ) {
        yield* markSkipped(
          input.type === "destiny_guest"
            ? "Destiny was already reported during this live session"
            : "A viewer surge was already reported during this live session",
        );
        return;
      }
      const alert: LivestreamAlertRecord = {
        alertId: randomUUID(),
        type: input.type,
        title: input.title,
        message: input.message,
        reason: input.reason,
        confidence: input.confidence,
        createdAt: now,
      };
      this.alertTimes.set(key, alert.createdAt);
      const alertStartedAt = now;
      yield* this.updateStage(
        input.observation.streamer.id,
        current.sessionStartedAt,
        "alert",
        {
          status: "running",
          eligible: true,
          startedAt: alertStartedAt,
          detail: alert.title,
        },
      );
      yield* notify({
        title: alert.title,
        message: `${alert.message}\n\nWhy: ${alert.reason}`,
        token: config.PUSHOVER_LIVE_TOKEN,
        ...getNotificationUrlFields(
          input.observation.status.primary.platform,
          input.observation.status.primary.username,
          input.observation.status.primary.urlOverride,
        ),
      }).pipe(
        Effect.mapError((cause) => new LivestreamNotificationError({ cause })),
        Effect.catch((error) => {
          if (this.alertTimes.get(key) === alert.createdAt) this.alertTimes.delete(key);
          return Effect.gen({ self: this }, function* () {
            const finishedAt = yield* Clock.currentTimeMillis;
            const detail = (
              error.cause instanceof Error ? error.cause.message : String(error.cause)
            ).slice(0, 500);
            yield* this.updateStage(
              input.observation.streamer.id,
              current.sessionStartedAt,
              "alert",
              {
                status: "error",
                eligible: true,
                startedAt: alertStartedAt,
                finishedAt,
                durationMs: finishedAt - alertStartedAt,
                detail,
              },
            );
            yield* this.recordEvent({
              streamerId: input.observation.streamer.id,
              sessionStartedAt: current.sessionStartedAt,
              kind: "alert",
              status: "error",
              title: "Alert delivery failed",
              detail,
            });
            return yield* Effect.fail(error);
          });
        }),
      );
      const latest = yield* this.currentSession(input.observation);
      if (!latest) return;
      const finishedAt = yield* Clock.currentTimeMillis;
      yield* saveLivestreamIntelligence({
        ...latest,
        latestAlert: alert,
        alertedAtByType: {
          ...latest.alertedAtByType,
          [alert.type]: alert.createdAt,
        },
        updatedAt: finishedAt,
      });
      yield* this.updateStage(
        input.observation.streamer.id,
        latest.sessionStartedAt,
        "alert",
        {
          status: "success",
          eligible: true,
          startedAt: alertStartedAt,
          finishedAt,
          durationMs: finishedAt - alertStartedAt,
          detail: alert.title,
          metrics: { type: alert.type, confidence: alert.confidence },
        },
      );
      yield* this.recordEvent({
        streamerId: input.observation.streamer.id,
        sessionStartedAt: latest.sessionStartedAt,
        kind: "alert",
        status: "success",
        title: `Alert sent: ${alert.title}`,
        detail: alert.reason,
        durationMs: finishedAt - alertStartedAt,
        metrics: { type: alert.type, confidence: alert.confidence },
      });
    });
  }
}

export function createLivestreamIntelligenceService(
  logger: NamedLogger,
): EffectType<LivestreamIntelligenceService | undefined, SpeechRecognitionError> {
  if (!config.LIVESTREAM_INTELLIGENCE_ENABLED) return Effect.succeed(undefined);
  return LocalSpeechRuntime.createEffect(
    config.LIVESTREAM_MODEL_DIR,
    config.LIVESTREAM_DESTINY_VOICEPRINT_PATH,
    config.LIVESTREAM_DESTINY_SPEAKER_THRESHOLD,
  ).pipe(
    Effect.map(
      (speech) =>
        new LivestreamIntelligenceService(logger.extend("LivestreamIntelligence"), {
          speech,
        }),
    ),
  );
}
