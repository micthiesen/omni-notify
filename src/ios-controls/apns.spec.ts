import { generateKeyPairSync, verify } from "node:crypto";
import { EventEmitter } from "node:events";
import type { ClientHttp2Session } from "node:http2";
import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";
import {
  ApnsControlClient,
  ApnsTransportError,
  buildApnsControlRequest,
  createApnsProviderToken,
} from "./apns.js";

const config = {
  teamId: "TEAM123",
  keyId: "KEY123",
  bundleId: "com.example.OmniLive",
  privateKeyPath: "/tmp/omni-notify-apns-key-does-not-exist.p8",
};

function privateKeyPem(): string {
  return generateKeyPairSync("ec", { namedCurve: "prime256v1" })
    .privateKey.export({ type: "pkcs8", format: "pem" })
    .toString();
}

describe("APNs control pushes", () => {
  it("builds an Apple controls request with the required topic and payload", () => {
    const request = buildApnsControlRequest("com.example.OmniLive", "abc", "jwt");
    expect(request.headers).toMatchObject({
      ":path": "/3/device/abc",
      authorization: "bearer jwt",
      "apns-push-type": "controls",
      "apns-topic": "com.example.OmniLive.push-type.controls",
      "apns-priority": "10",
      "apns-expiration": "0",
    });
    expect(JSON.parse(request.body)).toEqual({ aps: { "content-changed": true } });
  });

  it("creates a valid ES256 provider token", () => {
    const { privateKey, publicKey } = generateKeyPairSync("ec", {
      namedCurve: "prime256v1",
    });
    const pem = privateKey.export({ type: "pkcs8", format: "pem" }).toString();
    const token = createApnsProviderToken(
      { teamId: "TEAM123", keyId: "KEY123" },
      pem,
      123456,
    );
    const [header, claims, signature] = token.split(".");
    expect(JSON.parse(Buffer.from(header, "base64url").toString())).toEqual({
      alg: "ES256",
      kid: "KEY123",
    });
    expect(JSON.parse(Buffer.from(claims, "base64url").toString())).toEqual({
      iss: "TEAM123",
      iat: 123456,
    });
    expect(
      verify(
        "sha256",
        Buffer.from(`${header}.${claims}`),
        {
          key: publicKey,
          dsaEncoding: "ieee-p1363",
        },
        Buffer.from(signature, "base64url"),
      ),
    ).toBe(true);
  });

  it("reports an unreadable signing key as a typed construction failure", async () => {
    const error = await Effect.runPromise(
      ApnsControlClient.createEffect(config).pipe(Effect.flip),
    );

    expect(error).toBeInstanceOf(ApnsTransportError);
    expect(error.operation).toBe("signing key read");
    expect(error.message).toContain("ENOENT");
  });

  it("destroys an interrupted request and closes its scoped HTTP/2 session", async () => {
    const started = await Effect.runPromise(Deferred.make<void>());
    let requestDestroyed = 0;
    let sessionClosed = 0;
    const request = Object.assign(new EventEmitter(), {
      setEncoding: () => request,
      end: () => {
        Effect.runSync(Deferred.succeed(started, undefined));
      },
      destroy: () => {
        requestDestroyed += 1;
      },
    });
    const sessionEvents = new EventEmitter();
    let sessionIsClosed = false;
    const session = Object.assign(sessionEvents, {
      get closed() {
        return sessionIsClosed;
      },
      destroyed: false,
      request: () => request,
      close: () => {
        sessionClosed += 1;
        sessionIsClosed = true;
        sessionEvents.emit("close");
      },
    }) as unknown as ClientHttp2Session;
    const program = Effect.scoped(
      Effect.gen(function* () {
        const client = yield* ApnsControlClient.scoped(config, {
          readPrivateKey: () => Effect.succeed(privateKeyPem()),
          connect: () => session,
        });
        yield* client.sendControlChangedEffect({
          registrationId: "registration",
          deviceId: "device",
          controlId: "control",
          slot: 1,
          pushToken: "token",
          environment: "sandbox",
          createdAt: 1,
          updatedAt: 1,
        });
      }),
    );

    const fiber = Effect.runFork(program);
    await Effect.runPromise(Deferred.await(started));
    await Effect.runPromise(Fiber.interrupt(fiber));

    expect(requestDestroyed).toBe(1);
    expect(sessionClosed).toBe(1);
  });
});
