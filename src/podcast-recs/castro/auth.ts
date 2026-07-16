import { createHash, createHmac } from "node:crypto";

export interface CastroCredentials {
  accessId: string;
  secret: Uint8Array;
}

export interface CastroRequestToSign {
  method: string;
  pathAndQuery: string;
  date: string;
  body?: string;
  contentType?: string;
}

export interface CastroAuthHeaders {
  Authorization: string;
  "Content-Type": string;
  Date: string;
  "X-Authorization-Content-SHA256": string;
}

const DEFAULT_CONTENT_TYPE = "application/json";

export function hashCastroRequestBody(body = ""): string {
  return createHash("sha256").update(body).digest("base64");
}

export function createCastroCanonicalString(
  request: CastroRequestToSign,
  contentHash = hashCastroRequestBody(request.body),
): string {
  return [
    request.method.toUpperCase(),
    request.contentType ?? DEFAULT_CONTENT_TYPE,
    contentHash,
    request.pathAndQuery,
    request.date,
  ].join(",");
}

export function signCastroCanonicalString(
  canonicalString: string,
  secret: Uint8Array,
): string {
  return createHmac("sha256", secret).update(canonicalString).digest("base64");
}

export function createCastroAuthHeaders(
  credentials: CastroCredentials,
  request: CastroRequestToSign,
): CastroAuthHeaders {
  const contentHash = hashCastroRequestBody(request.body);
  const signature = signCastroCanonicalString(
    createCastroCanonicalString(request, contentHash),
    credentials.secret,
  );

  return {
    Authorization: `APIAuth-HMAC-SHA256 ${credentials.accessId}:${signature}`,
    "Content-Type": request.contentType ?? DEFAULT_CONTENT_TYPE,
    Date: request.date,
    "X-Authorization-Content-SHA256": contentHash,
  };
}
