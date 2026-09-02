import type { Effect as EffectType } from "effect/Effect";
import { createPrivateKey, sign } from "node:crypto";
import { readFile } from "node:fs/promises";
import { type ClientHttp2Session, connect } from "node:http2";
import { Data, Effect, Scope } from "effect";
import type { ApnsEnvironment, IOSControlRegistration } from "./persistence.js";

export type ApnsConfig = {
  teamId: string;
  keyId: string;
  bundleId: string;
  privateKeyPath: string;
};

export type ApnsControlPushResult =
  | { kind: "sent" }
  | { kind: "invalid-token"; reason: string }
  | { kind: "failed"; status: number; reason: string };

export class ApnsTransportError extends Data.TaggedError("ApnsTransportError")<{
  readonly operation?: string;
  readonly cause: unknown;
}> {
  public override get message(): string {
    const detail =
      this.cause instanceof Error ? this.cause.message : String(this.cause);
    return `APNs ${this.operation ?? "transport"} failed: ${detail}`;
  }
}

export interface ApnsClientDependencies {
  readonly readPrivateKey: (path: string) => EffectType<string, ApnsTransportError>;
  readonly connect: (authority: string) => ClientHttp2Session;
}

const defaultDependencies: ApnsClientDependencies = {
  readPrivateKey: (path) =>
    Effect.tryPromise({
      try: () => readFile(path, "utf8"),
      catch: (cause) =>
        new ApnsTransportError({ operation: "signing key read", cause }),
    }),
  connect,
};

export function buildApnsControlRequest(
  bundleId: string,
  pushToken: string,
  providerToken: string,
): { headers: Record<string, string | number>; body: string } {
  const body = JSON.stringify({ aps: { "content-changed": true } });
  return {
    headers: {
      ":method": "POST",
      ":path": `/3/device/${pushToken}`,
      authorization: `bearer ${providerToken}`,
      "apns-push-type": "controls",
      "apns-topic": `${bundleId}.push-type.controls`,
      "apns-priority": "10",
      "apns-expiration": "0",
      "content-type": "application/json",
      "content-length": Buffer.byteLength(body),
    },
    body,
  };
}

function base64Url(value: string | Buffer): string {
  return Buffer.from(value).toString("base64url");
}

export function createApnsProviderToken(
  config: Pick<ApnsConfig, "teamId" | "keyId">,
  privateKeyPem: string,
  issuedAt = Math.floor(Date.now() / 1000),
): string {
  const header = base64Url(JSON.stringify({ alg: "ES256", kid: config.keyId }));
  const claims = base64Url(JSON.stringify({ iss: config.teamId, iat: issuedAt }));
  const unsigned = `${header}.${claims}`;
  const signature = sign("sha256", Buffer.from(unsigned), {
    key: createPrivateKey(privateKeyPem),
    dsaEncoding: "ieee-p1363",
  });
  return `${unsigned}.${base64Url(signature)}`;
}

export class ApnsControlClient {
  private token?: { value: string; issuedAt: number };
  private sessions = new Map<ApnsEnvironment, ClientHttp2Session>();

  private constructor(
    private readonly config: ApnsConfig,
    private readonly privateKeyPem: string,
    private readonly dependencies: ApnsClientDependencies,
  ) {}

  public static createEffect(
    config: ApnsConfig,
    dependencies: ApnsClientDependencies = defaultDependencies,
  ): EffectType<ApnsControlClient, ApnsTransportError> {
    return dependencies
      .readPrivateKey(config.privateKeyPath)
      .pipe(
        Effect.map(
          (privateKeyPem) => new ApnsControlClient(config, privateKeyPem, dependencies),
        ),
      );
  }

  public static scoped(
    config: ApnsConfig,
    dependencies: ApnsClientDependencies = defaultDependencies,
  ): EffectType<ApnsControlClient, ApnsTransportError, Scope.Scope> {
    return Effect.acquireRelease(
      ApnsControlClient.createEffect(config, dependencies),
      (client) => client.closeEffect(),
    );
  }

  public close(): void {
    for (const session of this.sessions.values()) session.close();
    this.sessions.clear();
  }

  public closeEffect(): EffectType<void> {
    return Effect.sync(() => this.close());
  }

  public sendControlChangedEffect(
    registration: IOSControlRegistration,
  ): EffectType<ApnsControlPushResult, ApnsTransportError> {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.sessionEffect(registration.environment);
      const providerToken = yield* this.providerTokenEffect();
      const { headers, body } = buildApnsControlRequest(
        this.config.bundleId,
        registration.pushToken,
        providerToken,
      );
      return yield* Effect.callback<ApnsControlPushResult, ApnsTransportError>(
        (resume) => {
          let request: ReturnType<ClientHttp2Session["request"]>;
          try {
            request = session.request(headers);
          } catch (cause) {
            resume(
              Effect.fail(new ApnsTransportError({ operation: "request", cause })),
            );
            return;
          }
          let status = 0;
          let response = "";
          request.setEncoding("utf8");
          request.on("response", (headers) => {
            status = Number(headers[":status"] ?? 0);
          });
          request.on("data", (chunk) => {
            response += chunk;
          });
          request.on("end", () => {
            if (status === 200) return resume(Effect.succeed({ kind: "sent" }));
            let reason = response || `HTTP ${status}`;
            try {
              reason = JSON.parse(response).reason ?? reason;
            } catch {}
            if (
              status === 410 ||
              reason === "BadDeviceToken" ||
              reason === "DeviceTokenNotForTopic" ||
              reason === "Unregistered"
            ) {
              return resume(Effect.succeed({ kind: "invalid-token", reason }));
            }
            resume(Effect.succeed({ kind: "failed", status, reason }));
          });
          request.on("error", (cause) =>
            resume(
              Effect.fail(new ApnsTransportError({ operation: "request", cause })),
            ),
          );
          request.end(body);
          return Effect.sync(() => request.destroy());
        },
      );
    }).pipe(
      Effect.timeoutOrElse({
        duration: "5 seconds",
        orElse: () =>
          Effect.fail(
            new ApnsTransportError({
              operation: "request",
              cause: new Error("APNs control push timed out"),
            }),
          ),
      }),
    );
  }

  private providerTokenEffect(): EffectType<string, ApnsTransportError> {
    return Effect.try({
      try: () => {
        const issuedAt = Math.floor(Date.now() / 1000);
        if (!this.token || issuedAt - this.token.issuedAt >= 50 * 60) {
          this.token = {
            value: createApnsProviderToken(this.config, this.privateKeyPem, issuedAt),
            issuedAt,
          };
        }
        return this.token.value;
      },
      catch: (cause) =>
        new ApnsTransportError({ operation: "provider token creation", cause }),
    });
  }

  private sessionEffect(
    environment: ApnsEnvironment,
  ): EffectType<ClientHttp2Session, ApnsTransportError> {
    return Effect.try({
      try: () => {
        const current = this.sessions.get(environment);
        if (current && !current.closed && !current.destroyed) return current;
        const host =
          environment === "sandbox"
            ? "https://api.sandbox.push.apple.com"
            : "https://api.push.apple.com";
        const session = this.dependencies.connect(host);
        session.on("error", () => this.sessions.delete(environment));
        session.on("close", () => this.sessions.delete(environment));
        this.sessions.set(environment, session);
        return session;
      },
      catch: (cause) =>
        new ApnsTransportError({ operation: "HTTP/2 session creation", cause }),
    });
  }
}
