import { createPrivateKey, sign } from "node:crypto";
import { readFileSync } from "node:fs";
import { type ClientHttp2Session, connect } from "node:http2";
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
  private readonly privateKeyPem: string;
  private token?: { value: string; issuedAt: number };
  private sessions = new Map<ApnsEnvironment, ClientHttp2Session>();

  public constructor(private readonly config: ApnsConfig) {
    this.privateKeyPem = readFileSync(config.privateKeyPath, "utf8");
  }

  public close(): void {
    for (const session of this.sessions.values()) session.close();
    this.sessions.clear();
  }

  public async sendControlChanged(
    registration: IOSControlRegistration,
  ): Promise<ApnsControlPushResult> {
    const session = this.session(registration.environment);
    const { headers, body } = buildApnsControlRequest(
      this.config.bundleId,
      registration.pushToken,
      this.providerToken(),
    );
    return new Promise((resolve, reject) => {
      const request = session.request(headers);
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
        if (status === 200) return resolve({ kind: "sent" });
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
          return resolve({ kind: "invalid-token", reason });
        }
        resolve({ kind: "failed", status, reason });
      });
      request.on("error", reject);
      request.setTimeout(5_000, () => {
        request.destroy(new Error("APNs control push timed out"));
      });
      request.end(body);
    });
  }

  private providerToken(): string {
    const issuedAt = Math.floor(Date.now() / 1000);
    if (!this.token || issuedAt - this.token.issuedAt >= 50 * 60) {
      this.token = {
        value: createApnsProviderToken(this.config, this.privateKeyPem, issuedAt),
        issuedAt,
      };
    }
    return this.token.value;
  }

  private session(environment: ApnsEnvironment): ClientHttp2Session {
    const current = this.sessions.get(environment);
    if (current && !current.closed && !current.destroyed) return current;
    const host =
      environment === "sandbox"
        ? "https://api.sandbox.push.apple.com"
        : "https://api.push.apple.com";
    const session = connect(host);
    session.on("error", () => this.sessions.delete(environment));
    session.on("close", () => this.sessions.delete(environment));
    this.sessions.set(environment, session);
    return session;
  }
}
