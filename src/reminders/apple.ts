/*
 * SRP implementation adapted from ticaki/ioBroker.icloud v2.1.2.
 * MIT License
 * Copyright (c) 2026 ticaki <github@renopoint.de>
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
 * THE SOFTWARE.
 */
import crypto from "node:crypto";
import { Data, Effect, Schema } from "effect";
import { CookieJar } from "tough-cookie";

// ── Inline SRP implementation (Apple GSA mode, SHA-256, 2048-bit) ─────────────
// Previously provided by @foxt/js-srp (pure ESM). Inlined to remove the ESM
// dependency. All crypto operations use Node's built-in webcrypto (crypto.subtle).

/** 2048-bit MODP prime for SRP (RFC 5054 group 14 / Apple GSA). */
const SRP_N = BigInt(
  "0xac6bdb41324a9a9bf166de5e1389582faf72b6651987ee07fc3192943db56050a37329cbb4a099ed8193e0757767a13dd52312ab4b03310dcd7f48a9da04fd50e8083969edb767b0cf6095179a163ab3661a05fbd5faaae82918a9962f0b93b855f97993ec975eeaa80d740adbf4ff747359d041d5c33ea71d281e446b14773bca97b43a23fb801676bd207a436c6481f1d2b9078717461a5b9d32e688f87748544523b524b0d57d5ea77a2775d2ecfa032cfbdbf52fb3786160279004e57ae6af874e7303ce53299ccc041c7bc308d82a5698f3a8d0c38271ae35f8e9dbfbb694b5c803d89f7ae435de236d525f54759b65e372fcd68ef20fa7111f9e4aff73",
);
const SRP_G = 2n;
const SRP_N_BYTES = 256; // 2048-bit / 8

function srpBytesFromBigint(n: bigint): Uint8Array {
  let hex = n.toString(16);
  if (hex.length % 2) {
    hex = `0${hex}`;
  }
  const result = new Uint8Array(hex.length / 2);
  for (let i = 0; i < result.length; i++) {
    result[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return result;
}

function srpBigintFromBytes(bytes: Uint8Array): bigint {
  let n = 0n;
  for (const b of bytes) {
    n = (n << 8n) + BigInt(b);
  }
  return n;
}

function srpPadToLen(value: bigint, targetLen: number): Uint8Array {
  const raw = srpBytesFromBigint(value);
  if (raw.length >= targetLen) {
    return raw;
  }
  const padded = new Uint8Array(targetLen);
  padded.set(raw, targetLen - raw.length);
  return padded;
}

function srpConcat(...arrays: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const a of arrays) {
    total += a.length;
  }
  const result = new Uint8Array(total);
  let offset = 0;
  for (const a of arrays) {
    result.set(a, offset);
    offset += a.length;
  }
  return result;
}

function srpXor(a: Uint8Array, b: Uint8Array): Uint8Array {
  const result = new Uint8Array(a.length);
  for (let i = 0; i < a.length; i++) {
    result[i] = a[i] ^ b[i];
  }
  return result;
}

function srpToHex(bytes: Uint8Array): string {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function srpSha256(data: Uint8Array | ArrayBuffer): Promise<Uint8Array> {
  const buf = data instanceof ArrayBuffer ? Buffer.from(data) : Buffer.from(data);
  return new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", buf));
}

async function srpSha256BigInt(data: Uint8Array): Promise<bigint> {
  return srpBigintFromBytes(await srpSha256(data));
}

function srpModPow(base: bigint, exp: bigint, mod: bigint): bigint {
  if (mod === 1n) {
    return 0n;
  }
  base = ((base % mod) + mod) % mod;
  let result = 1n;
  for (; exp > 0n; exp >>= 1n) {
    if (exp & 1n) {
      result = (result * base) % mod;
    }
    base = (base * base) % mod;
  }
  return result;
}

// ─────────────────────────────────────────────────────────────────────────────

export type SRPProtocol = "s2k" | "s2k_fo";

export interface ServerSRPInitRequest {
  a: string;
  accountName: string;
  protocols: SRPProtocol[];
}
export interface ServerSRPInitResponse {
  iteration: number;
  salt: string;
  protocol: "s2k" | "s2k_fo";
  b: string;
  c: string;
}
export interface ServerSRPCompleteRequest {
  accountName: string;
  c: string;
  m1: string;
  m2: string;
  rememberMe: boolean;
  trustTokens: string[];
}

const stringToU8Array = (str: string): Uint8Array => new TextEncoder().encode(str);
const base64ToU8Array = (str: string): Uint8Array =>
  Uint8Array.from(Buffer.from(str, "base64"));

export class GSASRPAuthenticator {
  constructor(private username: string) {}

  private clienta?: bigint;
  private clientA?: bigint;

  private async derivePassword(
    protocol: "s2k" | "s2k_fo",
    password: string,
    salt: Uint8Array,
    iterations: number,
  ): Promise<Uint8Array> {
    let passHash = await srpSha256(stringToU8Array(password));
    if (protocol === "s2k_fo") {
      passHash = stringToU8Array(srpToHex(passHash));
    }

    const imported = await globalThis.crypto.subtle.importKey(
      "raw",
      Buffer.from(passHash),
      { name: "PBKDF2" },
      false,
      ["deriveBits"],
    );
    const derived = await globalThis.crypto.subtle.deriveBits(
      {
        name: "PBKDF2",
        hash: { name: "SHA-256" },
        iterations,
        salt: Buffer.from(salt),
      },
      imported,
      256,
    );
    return new Uint8Array(derived);
  }

  async getInit(): Promise<ServerSRPInitRequest> {
    if (this.clientA !== undefined) {
      throw new Error("Already initialized");
    }
    this.clienta = srpBigintFromBytes(crypto.randomBytes(SRP_N_BYTES));
    this.clientA = srpModPow(SRP_G, this.clienta, SRP_N);
    const a = Buffer.from(srpBytesFromBigint(this.clientA)).toString("base64");
    return { a, protocols: ["s2k", "s2k_fo"], accountName: this.username };
  }

  async getComplete(
    password: string,
    serverData: ServerSRPInitResponse,
  ): Promise<Pick<ServerSRPCompleteRequest, "m1" | "m2" | "c" | "accountName">> {
    if (this.clientA === undefined || this.clienta === undefined) {
      throw new Error("Not initialized");
    }
    if (serverData.protocol !== "s2k" && serverData.protocol !== "s2k_fo") {
      throw new Error(`Unsupported protocol ${serverData.protocol as string}`);
    }

    const salt = base64ToU8Array(serverData.salt);
    const serverPubBytes = base64ToU8Array(serverData.b);
    const B = srpBigintFromBytes(serverPubBytes);

    // k = sha256(N_bytes || pad(g, N_BYTES)) as bigint
    const k = await srpSha256BigInt(
      srpConcat(srpBytesFromBigint(SRP_N), srpPadToLen(SRP_G, SRP_N_BYTES)),
    );

    // u = sha256(pad(A, N_BYTES) || pad(B, N_BYTES)) as bigint
    const u = await srpSha256BigInt(
      srpConcat(srpPadToLen(this.clientA, SRP_N_BYTES), srpPadToLen(B, SRP_N_BYTES)),
    );

    // Derive password using PBKDF2
    const p = await this.derivePassword(
      serverData.protocol,
      password,
      salt,
      serverData.iteration,
    );

    // x = sha256(salt || sha256(":" || derived_password)) as bigint
    // (Apple GSA mode: identity bytes are empty, so inner hash is of ":" || p)
    const x = await srpSha256BigInt(
      srpConcat(salt, await srpSha256(srpConcat(new Uint8Array([58]), p))),
    );

    // S = (B - k*g^x)^(a + u*x) mod N
    const base = B - srpModPow(SRP_G, x, SRP_N) * k; // may be negative; modPow normalises
    const S = srpModPow(base, this.clienta + u * x, SRP_N);

    // K = sha256(S_bytes)
    const K = await srpSha256(srpBytesFromBigint(S));

    // M1 = sha256(XOR(sha256(N_bytes), sha256(pad(g,N))) || sha256(username) || salt || A_bytes || B_bytes || K)
    const M1 = await srpSha256(
      srpConcat(
        srpXor(
          await srpSha256(srpBytesFromBigint(SRP_N)),
          await srpSha256(srpPadToLen(SRP_G, SRP_N_BYTES)),
        ),
        await srpSha256(stringToU8Array(this.username)),
        salt,
        srpBytesFromBigint(this.clientA),
        srpBytesFromBigint(B),
        K,
      ),
    );

    // M2 = sha256(A_bytes || M1 || K)
    const M2 = await srpSha256(srpConcat(srpBytesFromBigint(this.clientA), M1, K));

    return {
      accountName: this.username,
      m1: Buffer.from(M1).toString("base64"),
      m2: Buffer.from(M2).toString("base64"),
      c: serverData.c,
    };
  }
}

const APPLE_WIDGET_KEY =
  "d39ba9916b7251055b22c7f910e2ea796ee65e98b2ddecea8f5dde8d9d1a815d";
const AUTH_ROOT = "https://idmsa.apple.com/appleauth/auth/";
const SETUP_ROOT = "https://setup.icloud.com/setup/ws/1/";
const CLOUDKIT_SUFFIX = "/database/1/com.apple.reminders/production/private";
const MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
// Apple-only exception approved by Michael: upstream documents 404/503 failures
// with incompatible User-Agents. Keep these protocol headers scoped to this client.
const AUTH_USER_AGENT =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";
const SERVICE_USER_AGENT = "python-requests/2.31.0";

export class AppleRemindersError extends Data.TaggedError("AppleRemindersError")<{
  readonly operation: string;
  readonly reason: string;
  readonly status?: number;
  readonly kind:
    | "authentication-needed"
    | "transient-outage"
    | "rate-limited"
    | "unsupported-protocol"
    | "awaiting-device-approval"
    | "terms-required";
}> {}

export interface AppleSession {
  clientId: string;
  scnt?: string;
  sessionId?: string;
  sessionToken?: string;
  trustToken?: string;
  accountCountry?: string;
  authAttributes?: string;
  cookies?: string;
  ckBaseUrl?: string;
  dsid?: string;
}

const AppleSessionSchema = Schema.Struct({
  clientId: Schema.String,
  scnt: Schema.optional(Schema.String),
  sessionId: Schema.optional(Schema.String),
  sessionToken: Schema.optional(Schema.String),
  trustToken: Schema.optional(Schema.String),
  accountCountry: Schema.optional(Schema.String),
  authAttributes: Schema.optional(Schema.String),
  cookies: Schema.optional(Schema.String),
  ckBaseUrl: Schema.optional(Schema.String),
  dsid: Schema.optional(Schema.String),
});

export interface AppleRemindersDependencies {
  account: string;
  password: string;
  loadSession: () => Effect.Effect<unknown, unknown>;
  saveSession: (session: AppleSession) => Effect.Effect<void, unknown>;
  fetch?: typeof fetch;
  timeoutMs?: number;
}

export type AppleBeginResult = "ready" | "mfa-required";
export type ApplePcsResult = "not-required" | "consent-required" | "ready";

const UnknownRecord = Schema.Record(Schema.String, Schema.Unknown);
function asRecord(value: unknown): Record<string, unknown> {
  return Schema.is(UnknownRecord)(value) ? value : {};
}
function stringField(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}
function validatedSrpChallenge(value: unknown): ServerSRPInitResponse {
  const data = asRecord(value);
  if (
    (data.protocol !== "s2k" && data.protocol !== "s2k_fo") ||
    !Number.isInteger(data.iteration) ||
    (data.iteration as number) < 1 ||
    (data.iteration as number) > 1_000_000 ||
    !["salt", "b", "c"].every(
      (key) => typeof data[key] === "string" && (data[key] as string).length < 4_096,
    )
  ) {
    throw new Error("Invalid SRP challenge");
  }
  const serverPublic = Buffer.from(data.b as string, "base64");
  if (
    serverPublic.length === 0 ||
    serverPublic.length > SRP_N_BYTES ||
    srpBigintFromBytes(serverPublic) % SRP_N === 0n
  ) {
    throw new Error("Invalid SRP public value");
  }
  return data as unknown as ServerSRPInitResponse;
}
function appleUrl(raw: string): URL {
  const url = new URL(raw);
  if (
    url.protocol !== "https:" ||
    url.username ||
    url.password ||
    url.port ||
    !(
      url.hostname === "icloud.com" ||
      url.hostname.endsWith(".icloud.com") ||
      url.hostname === "apple.com" ||
      url.hostname.endsWith(".apple.com")
    )
  ) {
    throw new Error("Untrusted Apple service URL");
  }
  return url;
}

/** Authentication and bounded CloudKit transport. No network work occurs at construction. */
export class AppleRemindersClient {
  private session: AppleSession | null = null;
  private jar = new CookieJar();
  private sms?: { phoneNumber: Record<string, unknown>; mode: string };
  private readonly fetchImpl: typeof fetch;
  private readonly timeoutMs: number;

  constructor(private readonly deps: AppleRemindersDependencies) {
    this.fetchImpl = deps.fetch ?? fetch;
    this.timeoutMs = deps.timeoutMs ?? 15_000;
  }

  private fail(operation: string, reason: string, status?: number) {
    const kind =
      status === 429 || status === 503
        ? "rate-limited"
        : status === 500 || status === 502 || status === 504
          ? "transient-outage"
          : status === 451
            ? "terms-required"
            : reason.includes("consent")
              ? "awaiting-device-approval"
              : status !== undefined &&
                  status !== 401 &&
                  status !== 403 &&
                  status !== 409
                ? "unsupported-protocol"
                : "authentication-needed";
    return new AppleRemindersError({ operation, reason, status, kind });
  }

  private load() {
    return Effect.gen({ self: this }, function* () {
      if (this.session) return this.session;
      const raw = yield* this.deps
        .loadSession()
        .pipe(Effect.mapError(() => this.fail("load session", "storage failed")));
      const loaded =
        raw == null
          ? null
          : yield* Effect.try({
              try: () => Schema.decodeUnknownSync(AppleSessionSchema)(raw),
              catch: () => this.fail("load session", "invalid stored session"),
            });
      this.session = loaded ?? { clientId: `auth-${crypto.randomUUID()}` };
      if (loaded?.cookies) {
        const serializedCookies = loaded.cookies;
        this.jar = yield* Effect.try({
          try: () => CookieJar.deserializeSync(JSON.parse(serializedCookies)),
          catch: () => this.fail("load session", "invalid stored cookies"),
        });
      }
      return this.session;
    });
  }

  private persist() {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.load();
      session.cookies = JSON.stringify(this.jar.serializeSync());
      yield* this.deps
        .saveSession({ ...session })
        .pipe(Effect.mapError(() => this.fail("save session", "storage failed")));
    });
  }

  private async requestRaw(
    rawUrl: string,
    method: "GET" | "POST" | "PUT",
    body: unknown,
    headers: Record<string, string>,
    signal: AbortSignal,
  ): Promise<{ status: number; data: unknown; response: Response }> {
    const url = appleUrl(rawUrl);
    const isAuth = url.hostname === "idmsa.apple.com";
    const isSrp = url.pathname.startsWith("/appleauth/auth/signin/");
    const cookies = this.jar.getCookieStringSync(url.toString());
    {
      const response = await this.fetchImpl(url, {
        method,
        redirect: "manual",
        signal: AbortSignal.any([signal, AbortSignal.timeout(this.timeoutMs)]),
        headers: {
          "User-Agent": isAuth ? AUTH_USER_AGENT : SERVICE_USER_AGENT,
          Accept: "application/json",
          Origin: "https://www.icloud.com",
          Referer:
            isAuth && !isSrp ? "https://idmsa.apple.com" : "https://www.icloud.com/",
          ...(body === undefined ? {} : { "Content-Type": "application/json" }),
          ...(cookies ? { Cookie: cookies } : {}),
          ...headers,
        },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      });
      for (const cookie of response.headers.getSetCookie()) {
        this.jar.setCookieSync(cookie, url.toString());
      }
      if (response.status >= 300 && response.status < 400) {
        throw new Error("Redirect rejected");
      }
      const length = Number(response.headers.get("content-length") ?? 0);
      if (length > MAX_RESPONSE_BYTES) throw new Error("Response too large");
      const chunks: Uint8Array[] = [];
      let bytes = 0;
      const reader = response.body?.getReader();
      if (reader) {
        while (true) {
          const part = await reader.read();
          if (part.done) break;
          bytes += part.value.byteLength;
          if (bytes > MAX_RESPONSE_BYTES) {
            await reader.cancel();
            throw new Error("Response too large");
          }
          chunks.push(part.value);
        }
      }
      const raw = Buffer.concat(chunks).toString("utf8");
      let data: unknown = null;
      if (raw.trim()) {
        try {
          data = JSON.parse(raw);
        } catch {
          if (response.ok) throw new Error("Invalid JSON response");
        }
      }
      return { status: response.status, data, response };
    }
  }

  private request(
    operation: string,
    url: string,
    method: "GET" | "POST" | "PUT" = "POST",
    body?: unknown,
    headers: Record<string, string> = {},
  ) {
    return Effect.tryPromise({
      try: (signal) => this.requestRaw(url, method, body, headers, signal),
      catch: (error) =>
        new AppleRemindersError({
          operation,
          reason: "Apple request failed",
          kind:
            error instanceof Error &&
            [
              "Invalid JSON response",
              "Redirect rejected",
              "Untrusted Apple service URL",
              "Response too large",
            ].includes(error.message)
              ? "unsupported-protocol"
              : "transient-outage",
        }),
    }).pipe(
      Effect.tap(({ response }) =>
        Effect.gen({ self: this }, function* () {
          if (this.session) this.captureAuth(response, this.session);
          yield* this.persist();
        }),
      ),
    );
  }

  private captureAuth(response: Response, session: AppleSession): void {
    const fields = [
      ["scnt", "scnt"],
      ["x-apple-id-session-id", "sessionId"],
      ["x-apple-session-token", "sessionToken"],
      ["x-apple-twosv-trust-token", "trustToken"],
      ["x-apple-id-account-country", "accountCountry"],
      ["x-apple-auth-attributes", "authAttributes"],
    ] as const;
    for (const [header, key] of fields) {
      const value = response.headers.get(header);
      if (value) session[key] = value;
    }
  }

  private authHeaders(session: AppleSession): Record<string, string> {
    return {
      "X-Apple-Widget-Key": APPLE_WIDGET_KEY,
      "X-Apple-OAuth-Client-Id": APPLE_WIDGET_KEY,
      "X-Apple-OAuth-Client-Type": "firstPartyAuth",
      "X-Apple-OAuth-Redirect-URI": "https://www.icloud.com",
      "X-Apple-OAuth-Require-Grant-Code": "true",
      "X-Apple-OAuth-Response-Type": "code",
      "X-Apple-OAuth-Response-Mode": "web_message",
      "X-Apple-OAuth-State": session.clientId,
      "X-Apple-Frame-Id": session.clientId,
      ...(session.scnt ? { scnt: session.scnt } : {}),
      ...(session.sessionId ? { "X-Apple-ID-Session-Id": session.sessionId } : {}),
      ...(session.authAttributes
        ? { "X-Apple-Auth-Attributes": session.authAttributes }
        : {}),
    };
  }

  private accountLogin() {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.load();
      if (!session.sessionToken) {
        return yield* this.fail("account login", "session token missing");
      }
      const response = yield* this.request(
        "account login",
        `${SETUP_ROOT}accountLogin`,
        "POST",
        {
          accountCountryCode: session.accountCountry,
          dsWebAuthToken: session.sessionToken,
          extended_login: true,
          trustToken: session.trustToken ?? "",
        },
      );
      this.captureAuth(response.response, session);
      if (response.status !== 200) {
        return yield* this.fail(
          "account login",
          "Apple rejected session",
          response.status,
        );
      }
      const data = asRecord(response.data);
      if (data.termsUpdateNeeded === true) {
        return yield* this.fail("account login", "terms acceptance required", 451);
      }
      const dsInfo = asRecord(data.dsInfo);
      if (
        Number(dsInfo.hsaVersion ?? 0) >= 2 &&
        (data.hsaChallengeRequired === true || data.hsaTrustedBrowser === false)
      ) {
        yield* this.persist();
        return false;
      }
      const services = asRecord(data.webservices);
      const ck = asRecord(services.ckdatabasews);
      const ckUrl = stringField(ck.url);
      if (!ckUrl) return yield* this.fail("account login", "CloudKit unavailable");
      const trusted = yield* Effect.try({
        try: () => appleUrl(ckUrl),
        catch: () => this.fail("account login", "untrusted CloudKit URL"),
      });
      session.ckBaseUrl = `${trusted.toString().replace(/\/$/, "")}${CLOUDKIT_SUFFIX}`;
      const dsid = asRecord(data.dsInfo).dsid;
      if (typeof dsid === "string" || typeof dsid === "number") {
        session.dsid = String(dsid);
      }
      yield* this.persist();
      return true;
    });
  }

  /** Validate the saved token without starting interactive sign-in. */
  verify() {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.load();
      if (!session.sessionToken) return false;
      const response = yield* this.request(
        "validate session",
        `${SETUP_ROOT}validate`,
        "POST",
        null,
      );
      this.captureAuth(response.response, session);
      if ([401, 403, 421].includes(response.status)) return false;
      if (response.status !== 200) {
        return yield* this.fail(
          "validate session",
          "Apple rejected validation",
          response.status,
        );
      }
      const data = asRecord(response.data);
      if (data.termsUpdateNeeded === true) {
        return yield* this.fail("validate session", "terms acceptance required", 451);
      }
      const dsInfo = asRecord(data.dsInfo);
      if (
        Number(dsInfo.hsaVersion ?? 0) >= 2 &&
        (data.hsaChallengeRequired === true || data.hsaTrustedBrowser === false)
      )
        return false;
      const services = asRecord(data.webservices);
      const ckUrl = stringField(asRecord(services.ckdatabasews).url);
      if (!ckUrl) return false;
      const trusted = yield* Effect.try({
        try: () => appleUrl(ckUrl),
        catch: () => this.fail("validate session", "untrusted CloudKit URL"),
      });
      session.ckBaseUrl = `${trusted.toString().replace(/\/$/, "")}${CLOUDKIT_SUFFIX}`;
      if (typeof dsInfo.dsid === "string" || typeof dsInfo.dsid === "number") {
        session.dsid = String(dsInfo.dsid);
      }
      yield* this.persist();
      return true;
    });
  }

  /** Start explicit SRP sign-in. The caller owns the second-factor step. */
  begin(): Effect.Effect<AppleBeginResult, AppleRemindersError> {
    return Effect.gen({ self: this }, function* () {
      if (!this.deps.account || !this.deps.password) {
        return yield* this.fail("begin", "Apple account is not configured");
      }
      if (yield* this.verify()) return "ready" as const;
      const session = yield* this.load();
      let srp = new GSASRPAuthenticator(this.deps.account);
      let first: Awaited<ReturnType<typeof this.requestRaw>> | undefined;
      for (let attempt = 0; attempt < 2; attempt++) {
        const init = yield* Effect.tryPromise({
          try: () => srp.getInit(),
          catch: () => this.fail("SRP init", "cryptography failed"),
        });
        first = yield* this.request(
          "SRP init",
          `${AUTH_ROOT}signin/init`,
          "POST",
          init,
          this.authHeaders(session),
        );
        if (first.status !== 409 || attempt === 1) break;
        session.scnt = undefined;
        session.sessionId = undefined;
        session.sessionToken = undefined;
        session.authAttributes = undefined;
        this.jar.removeAllCookiesSync();
        yield* this.persist();
        srp = new GSASRPAuthenticator(this.deps.account);
      }
      if (!first) return yield* this.fail("SRP init", "no response");
      if (first.status !== 200) {
        yield* this.persist();
        return yield* this.fail("SRP init", "Apple rejected sign-in", first.status);
      }
      const proof = yield* Effect.tryPromise({
        try: () =>
          srp.getComplete(this.deps.password, validatedSrpChallenge(first.data)),
        catch: () =>
          new AppleRemindersError({
            operation: "SRP proof",
            reason: "unsupported or invalid challenge",
            kind: "unsupported-protocol",
          }),
      });
      const second = yield* this.request(
        "SRP complete",
        `${AUTH_ROOT}signin/complete?isRememberMeEnabled=true`,
        "POST",
        {
          ...proof,
          trustTokens: session.trustToken ? [session.trustToken] : [],
          rememberMe: true,
        },
        this.authHeaders(session),
      );
      this.captureAuth(second.response, session);
      yield* this.persist();
      if (second.status !== 200 && second.status !== 409) {
        return yield* this.fail(
          "SRP complete",
          "Apple rejected sign-in",
          second.status,
        );
      }
      if (session.sessionToken) {
        const trusted = yield* this.accountLogin().pipe(
          Effect.catch((error) =>
            second.status === 409 &&
            error.operation === "account login" &&
            error.status !== undefined &&
            [401, 403, 421].includes(error.status)
              ? Effect.succeed(false)
              : Effect.fail(error),
          ),
        );
        if (trusted) return "ready" as const;
      }
      const options = yield* this.request(
        "MFA options",
        AUTH_ROOT.slice(0, -1),
        "GET",
        undefined,
        this.authHeaders(session),
      );
      this.captureAuth(options.response, session);
      if (options.status !== 200) {
        return yield* this.fail(
          "MFA options",
          "Apple challenge unavailable",
          options.status,
        );
      }
      if (asRecord(options.data).fsaChallenge) {
        return yield* new AppleRemindersError({
          operation: "MFA options",
          reason: "security-key authentication is required",
          kind: "unsupported-protocol",
        });
      }
      const push = yield* this.request(
        "MFA push",
        // PUT requests delivery; POST to this same path verifies the code.
        // The parent /verify/trusteddevice path rejects PUT with HTTP 405.
        `${AUTH_ROOT}verify/trusteddevice/securitycode`,
        "PUT",
        undefined,
        this.authHeaders(session),
      );
      // ioBroker continues to code entry when the optional device push fails.
      // Limit that fallback to method-not-allowed with a usable MFA challenge;
      // authentication, outage and rate-limit failures must remain visible.
      const codeEntryAvailable =
        push.status === 405 && Boolean(session.scnt && session.sessionId);
      // Apple may accept asynchronous popup delivery with 202.
      if (![200, 202, 204].includes(push.status) && !codeEntryAvailable) {
        return yield* this.fail(
          "MFA push",
          "Apple rejected device notification",
          push.status,
        );
      }
      yield* this.persist();
      return "mfa-required" as const;
    });
  }

  requestSmsCode() {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.load();
      const options = yield* this.request(
        "MFA options",
        AUTH_ROOT.slice(0, -1),
        "GET",
        undefined,
        this.authHeaders(session),
      );
      if (options.status !== 200) {
        return yield* this.fail(
          "MFA options",
          "Apple challenge unavailable",
          options.status,
        );
      }
      const info = asRecord(options.data);
      const phone = asRecord(info.trustedPhoneNumber);
      if (typeof phone.id !== "number" && typeof phone.id !== "string") {
        return yield* this.fail("SMS request", "trusted phone unavailable");
      }
      const phoneNumber = {
        id: phone.id,
        ...(typeof phone.nonFTEU === "boolean" ? { nonFTEU: phone.nonFTEU } : {}),
      };
      const response = yield* this.request(
        "SMS request",
        `${AUTH_ROOT}verify/phone`,
        "PUT",
        { phoneNumber, mode: "sms" },
        this.authHeaders(session),
      );
      if (response.status !== 200 && response.status !== 204) {
        return yield* this.fail(
          "SMS request",
          "Apple rejected request",
          response.status,
        );
      }
      this.sms = {
        phoneNumber,
        mode: stringField(asRecord(response.data).mode) ?? "sms",
      };
    });
  }

  submit2fa(code: string, channel: "device" | "sms" = "device") {
    return Effect.gen({ self: this }, function* () {
      if (!/^\d{6}$/.test(code)) {
        return yield* this.fail("MFA verify", "invalid six-digit code");
      }
      const session = yield* this.load();
      if (!session.scnt || !session.sessionId) {
        return yield* this.fail("MFA verify", "MFA challenge missing");
      }
      if (channel === "sms" && !this.sms) {
        return yield* this.fail("MFA verify", "request SMS code first");
      }
      const endpoint =
        channel === "device"
          ? "verify/trusteddevice/securitycode"
          : "verify/phone/securitycode";
      const body =
        channel === "device"
          ? { securityCode: { code } }
          : {
              phoneNumber: this.sms!.phoneNumber,
              securityCode: { code },
              mode: this.sms!.mode,
            };
      const response = yield* this.request(
        "MFA verify",
        `${AUTH_ROOT}${endpoint}`,
        "POST",
        body,
        this.authHeaders(session),
      );
      this.captureAuth(response.response, session);
      // Modern Apple code verification can report success as HTTP 409.
      // Require explicit validity and a token on this response, never a saved
      // token or the conflict status alone. Trust/account checks still follow.
      const codeValid = asRecord(asRecord(response.data).securityCode).valid;
      const acceptedConflict =
        response.status === 409 &&
        codeValid === true &&
        Boolean(response.response.headers.get("x-apple-session-token")?.trim());
      if (
        codeValid === false ||
        (response.status !== 200 && response.status !== 204 && !acceptedConflict)
      ) {
        yield* this.persist();
        return yield* this.fail("MFA verify", "Apple rejected code", response.status);
      }
      const trust = yield* this.request(
        "trust browser",
        `${AUTH_ROOT}2sv/trust`,
        "GET",
        undefined,
        this.authHeaders(session),
      );
      this.captureAuth(trust.response, session);
      if (trust.status !== 200 && trust.status !== 204) {
        return yield* this.fail("trust browser", "Apple rejected trust", trust.status);
      }
      yield* this.persist();
      const ready = yield* this.accountLogin();
      if (!ready) return yield* this.fail("MFA verify", "session still requires MFA");
      return "ready" as const;
    });
  }

  requestPcsAccess(): Effect.Effect<ApplePcsResult, AppleRemindersError> {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.load();
      if (!session.sessionToken) return yield* this.fail("PCS", "session missing");
      const params = new URLSearchParams({
        clientBuildNumber: "2534Project66",
        clientMasteringNumber: "2534B22",
        clientId: session.clientId,
        ...(session.dsid ? { dsid: session.dsid } : {}),
      });
      const state = yield* this.request(
        "PCS state",
        `${SETUP_ROOT}requestWebAccessState?${params}`,
      );
      if (state.status !== 200) {
        return yield* this.fail("PCS state", "Apple rejected request", state.status);
      }
      const info = asRecord(state.data);
      if (info.isICDRSDisabled !== true) return "not-required" as const;
      if (info.isDeviceConsentedForPCS !== true) {
        const consent = yield* this.request(
          "PCS consent",
          `${SETUP_ROOT}enableDeviceConsentForPCS?${params}`,
        );
        if (consent.status !== 200) {
          return yield* this.fail(
            "PCS consent",
            "Apple rejected request",
            consent.status,
          );
        }
        if (asRecord(consent.data).isDeviceConsentNotificationSent !== true)
          return yield* this.fail(
            "PCS consent",
            "Apple did not confirm the consent notification",
            consent.status,
          );
        return "consent-required" as const;
      }
      const cookies = yield* this.request(
        "PCS cookies",
        `${SETUP_ROOT}requestPCS?${params}`,
        "POST",
        { appName: "reminders", derivedFromUserAction: true },
      );
      const pcs = asRecord(cookies.data);
      if (
        cookies.status === 200 &&
        [
          "Requested the device to upload cookies.",
          "Cookies not available yet on server.",
        ].includes(String(pcs.message))
      )
        return "consent-required" as const;
      if (cookies.status !== 200 || pcs.status !== "success") {
        return yield* this.fail(
          "PCS cookies",
          "Apple has not granted access",
          cookies.status,
        );
      }
      yield* this.persist();
      return "ready" as const;
    });
  }

  ckPost(path: "/changes/zone" | "/records/lookup" | "/records/modify", body: unknown) {
    return Effect.gen({ self: this }, function* () {
      const session = yield* this.load();
      if (!session.ckBaseUrl) return yield* this.fail("CloudKit", "session not ready");
      const call = () =>
        this.request(
          "CloudKit",
          `${session.ckBaseUrl}${path}?remapEnums=true&getCurrentSyncToken=true`,
          "POST",
          body,
          { "Content-Type": "text/plain" },
        );
      let response = yield* call();
      if (response.status === 401 && path !== "/records/modify") {
        if (!(yield* this.accountLogin()))
          return yield* this.fail("CloudKit", "session requires verification", 401);
        response = yield* call();
      }
      if (response.status !== 200) {
        return yield* this.fail("CloudKit", "Apple rejected request", response.status);
      }
      const data = asRecord(response.data);
      if (data.error) return yield* this.fail("CloudKit", "Apple reported an error");
      const errors = [
        data,
        ...(Array.isArray(data.records) ? data.records.map(asRecord) : []),
        ...(Array.isArray(data.zones)
          ? data.zones.map((zone) => asRecord(asRecord(zone).error))
          : []),
      ];
      for (const error of errors) {
        if (
          ["AUTHENTICATION_REQUIRED", "AUTHENTICATION_FAILED"].includes(
            String(error.serverErrorCode),
          )
        )
          return yield* this.fail("CloudKit", "session requires verification", 401);
        if (["THROTTLED", "RATE_LIMITED"].includes(String(error.serverErrorCode)))
          return yield* this.fail("CloudKit", "request throttled", 429);
      }
      yield* this.persist();
      return data;
    });
  }
}
