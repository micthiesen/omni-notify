import { generateKeyPairSync, verify } from "node:crypto";
import { describe, expect, it } from "vitest";
import { buildApnsControlRequest, createApnsProviderToken } from "./apns.js";

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
});
