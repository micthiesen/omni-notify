import { createHash, randomBytes } from "node:crypto";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { ElicitRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { Effect } from "effect";
import type { AdapterOptions } from "./auth.js";
import {
  closeLegacyClient,
  openLegacyClient,
  type ElicitationSupport,
} from "./legacy.js";

type Input = { method: "elicitation/create"; params: Record<string, unknown> };
type Outcome =
  | { kind: "input"; input: Input }
  | { kind: "result"; result: unknown }
  | { kind: "error" };
type Pending = {
  readonly input: Input;
  readonly resolve: (value: {
    action: "accept" | "decline" | "cancel";
    content?: Record<string, unknown>;
  }) => void;
};
type Continuation = {
  readonly client: Client;
  readonly owner: string;
  readonly digest: string;
  readonly abort: AbortController;
  readonly queue: Outcome[];
  pending?: Pending;
  wake?: (value: Outcome) => void;
  timer?: NodeJS.Timeout;
  requestState?: string;
  closed?: boolean;
};

export class ContinuationError extends Error {
  constructor(readonly kind: "invalid" | "upstream" | "capacity") {
    super(kind);
  }
}

function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value !== null && typeof value === "object") {
    return `{${Object.entries(value)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([k, v]) => `${JSON.stringify(k)}:${canonical(v)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function argsDigest(params: Record<string, unknown>): string {
  return createHash("sha256")
    .update(canonical({ name: params.name, arguments: params.arguments ?? {} }))
    .digest("hex");
}

function next(continuation: Continuation): Promise<Outcome> {
  const queued = continuation.queue.shift();
  if (queued) return Promise.resolve(queued);
  return new Promise((resolve) => {
    continuation.wake = resolve;
  });
}

function push(continuation: Continuation, outcome: Outcome): void {
  if (continuation.wake) {
    const wake = continuation.wake;
    continuation.wake = undefined;
    wake(outcome);
  } else continuation.queue.push(outcome);
}

function answer(
  value: unknown,
  input: Input,
): {
  action: "accept" | "decline" | "cancel";
  content?: Record<string, unknown>;
} | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const item = value as Record<string, unknown>;
  if (item.action !== "accept" && item.action !== "decline" && item.action !== "cancel")
    return null;
  if (item.action === "accept") {
    if (input.params.mode === "url" && item.content === undefined)
      return { action: "accept" };
    if (
      !item.content ||
      typeof item.content !== "object" ||
      Array.isArray(item.content)
    )
      return null;
    return { action: "accept", content: item.content as Record<string, unknown> };
  }
  return { action: item.action };
}

export class Continuations {
  private readonly entries = new Map<string, Continuation>();
  private active = 0;
  constructor(
    private readonly options: AdapterOptions,
    private readonly ttlMs = 300_000,
    private readonly connect: typeof openLegacyClient = openLegacyClient,
    private readonly maxActive = 32,
  ) {}

  private async close(entry: Continuation): Promise<void> {
    if (entry.closed) return;
    entry.closed = true;
    this.active--;
    if (entry.timer) clearTimeout(entry.timer);
    if (entry.requestState) this.entries.delete(entry.requestState);
    entry.abort.abort();
    entry.pending?.resolve({ action: "cancel" });
    entry.pending = undefined;
    await closeLegacyClient(entry.client);
  }

  private expire(entry: Continuation): void {
    if (entry.closed) return;
    void this.close(entry);
    push(entry, { kind: "error" });
  }

  private render(entry: Continuation, outcome: Outcome): unknown {
    if (outcome.kind === "result") {
      void this.close(entry);
      return { resultType: "complete", ...(outcome.result as object) };
    }
    if (outcome.kind === "error") {
      void this.close(entry);
      throw new ContinuationError("upstream");
    }
    const state = randomBytes(32).toString("base64url");
    this.entries.set(state, entry);
    entry.requestState = state;
    return {
      resultType: "input_required",
      inputRequests: { elicitation: outcome.input },
      requestState: state,
    };
  }

  /** One legacy invocation continues across modern request/response rounds. */
  call(
    owner: string,
    authorization: string,
    query: URLSearchParams,
    params: Record<string, unknown>,
    support: ElicitationSupport = { form: true, url: false },
  ) {
    return Effect.tryPromise({
      try: async () => {
        const expiresAt = Date.now() + this.ttlMs;
        const state = params.requestState;
        const digest = argsDigest(params);
        if (typeof state === "string") {
          const entry = this.entries.get(state);
          if (
            !entry ||
            entry.owner !== owner ||
            entry.digest !== digest ||
            !entry.pending
          ) {
            throw new ContinuationError("invalid");
          }
          const responses = params.inputResponses;
          const response =
            responses && typeof responses === "object" && !Array.isArray(responses)
              ? answer(
                  (responses as Record<string, unknown>).elicitation,
                  entry.pending.input,
                )
              : null;
          if (!response) throw new ContinuationError("invalid");
          // Claim before waking the original invocation. Replay and concurrent
          // retries cannot execute the accepted input twice.
          this.entries.delete(state);
          entry.requestState = undefined;
          const pending = entry.pending;
          entry.pending = undefined;
          const nextOutcome = next(entry);
          pending.resolve(response);
          return this.render(entry, await nextOutcome);
        }
        if (
          params.inputResponses !== undefined ||
          typeof params.name !== "string" ||
          !params.name ||
          (params.arguments !== undefined &&
            (!params.arguments ||
              typeof params.arguments !== "object" ||
              Array.isArray(params.arguments)))
        )
          throw new ContinuationError("invalid");
        if (this.active >= this.maxActive) throw new ContinuationError("capacity");
        this.active++;
        let entry!: Continuation;
        let client: Client;
        try {
          client = await Effect.runPromise(
            this.connect(this.options, authorization, query, support, (newClient) => {
              newClient.setRequestHandler(
                ElicitRequestSchema,
                async (request) =>
                  new Promise((resolve) => {
                    const input: Input = {
                      method: "elicitation/create",
                      params: request.params as Record<string, unknown>,
                    };
                    entry.pending = { input, resolve };
                    push(entry, { kind: "input", input });
                  }),
              );
            }),
          );
        } catch {
          this.active--;
          throw new ContinuationError("upstream");
        }
        const abort = new AbortController();
        entry = { client, owner, digest, abort, queue: [] };
        if (Date.now() >= expiresAt) {
          await this.close(entry);
          throw new ContinuationError("upstream");
        }
        entry.timer = setTimeout(() => this.expire(entry), expiresAt - Date.now());
        entry.timer.unref();
        const first = next(entry);
        const tool = client
          .callTool(
            {
              name: params.name,
              arguments: params.arguments as Record<string, unknown> | undefined,
            },
            undefined,
            {
              signal: abort.signal,
              timeout: Math.max(1, expiresAt - Date.now()),
            },
          )
          .then(
            (result): Outcome => ({ kind: "result", result }),
            (): Outcome => ({ kind: "error" }),
          );
        tool.then((outcome) => {
          if (!entry.closed) push(entry, outcome);
        });
        return this.render(entry, await first);
      },
      catch: (cause) =>
        cause instanceof ContinuationError ? cause : new ContinuationError("upstream"),
    });
  }
}
