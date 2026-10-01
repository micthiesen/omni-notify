import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { Effect } from "effect";
import { authenticateOAuth, type AdapterOptions } from "./auth.js";
import { Continuations } from "./continuations.js";
import { callLegacy, closeLegacyClient, openLegacyClient } from "./legacy.js";

const VERSION = "2026-07-28";
const USER_AGENT = "OpenAI File Downloader, XaiImageApiFetch/1.0";
const SERVER_INFO = { name: "Executor with Omni Events", version: "0.1.0" };
const HOP_HEADERS = new Set([
  "connection",
  "keep-alive",
  "proxy-authenticate",
  "proxy-authorization",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

interface RpcRequest {
  readonly jsonrpc: "2.0";
  readonly id: string | number;
  readonly method: string;
  readonly params?: Record<string, unknown>;
}

function rpcRequest(raw: Buffer): RpcRequest | null {
  try {
    const value: unknown = JSON.parse(raw.toString("utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value)) return null;
    const data = value as Record<string, unknown>;
    if (
      data.jsonrpc !== "2.0" ||
      (typeof data.id !== "string" && typeof data.id !== "number") ||
      typeof data.method !== "string"
    )
      return null;
    if (
      data.params !== undefined &&
      (!data.params || typeof data.params !== "object" || Array.isArray(data.params))
    )
      return null;
    return data as unknown as RpcRequest;
  } catch {
    return null;
  }
}

function json(
  res: ServerResponse,
  status: number,
  value: unknown,
  extra: Record<string, string> = {},
): void {
  res.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
    ...extra,
  });
  res.end(JSON.stringify(value));
}

function error(
  res: ServerResponse,
  id: string | number | null,
  code: number,
  message: string,
  status = 200,
  data?: unknown,
  extra?: Record<string, string>,
): void {
  json(
    res,
    status,
    {
      jsonrpc: "2.0",
      id,
      error: { code, message, ...(data === undefined ? {} : { data }) },
    },
    extra,
  );
}

function result(
  res: ServerResponse,
  id: string | number,
  body: Record<string, unknown>,
): void {
  json(res, 200, {
    jsonrpc: "2.0",
    id,
    result: {
      resultType: "complete",
      ...body,
      _meta: {
        ...(body._meta as object | undefined),
        "io.modelcontextprotocol/serverInfo": SERVER_INFO,
      },
    },
  });
}

function readBody(req: IncomingMessage) {
  return Effect.tryPromise({
    try: async () => {
      const chunks: Buffer[] = [];
      let total = 0;
      for await (const chunk of req) {
        const part = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
        total += part.length;
        if (total > 4 * 1024 * 1024) throw new Error("Request too large");
        chunks.push(part);
      }
      return Buffer.concat(chunks);
    },
    catch: () => new Error("Invalid request body"),
  });
}

function proxyLegacy(
  req: IncomingMessage,
  res: ServerResponse,
  target: URL,
  raw: Buffer | undefined,
) {
  return Effect.tryPromise({
    try: async () => {
      const headers = new Headers();
      for (const [key, value] of Object.entries(req.headers)) {
        if (!HOP_HEADERS.has(key) && key !== "host" && value !== undefined)
          headers.set(key, Array.isArray(value) ? value.join(", ") : value);
      }
      headers.set("user-agent", USER_AGENT);
      const abort = new AbortController();
      const disconnect = () => {
        if (!res.writableEnded) abort.abort();
      };
      res.once("close", disconnect);
      req.once("aborted", disconnect);
      const upstream = await fetch(target, {
        method: req.method,
        headers,
        body: raw === undefined ? undefined : new Uint8Array(raw),
        redirect: "manual",
        signal: AbortSignal.any([abort.signal, AbortSignal.timeout(3600000)]),
      });
      const responseHeaders: Record<string, string> = {};
      for (const [key, value] of upstream.headers)
        if (!HOP_HEADERS.has(key) && key !== "content-length")
          responseHeaders[key] = value;
      res.writeHead(upstream.status, responseHeaders);
      try {
        if (upstream.body)
          await pipeline(
            Readable.fromWeb(
              upstream.body as import("node:stream/web").ReadableStream<Uint8Array>,
            ),
            res,
            { signal: abort.signal },
          );
        else res.end();
      } finally {
        res.off("close", disconnect);
        req.off("aborted", disconnect);
      }
    },
    catch: () => new Error("Executor proxy unavailable"),
  });
}

function forwardEvent(
  req: IncomingMessage,
  res: ServerResponse,
  raw: Buffer,
  owner: string,
  authorization: string,
  options: AdapterOptions,
) {
  return Effect.tryPromise({
    try: async () => {
      const upstream = await fetch(new URL("/mcp", options.omniBaseUrl), {
        method: "POST",
        headers: {
          authorization: `Bearer ${options.omniMcpToken}`,
          "content-type": "application/json",
          accept: "application/json, text/event-stream",
          "mcp-protocol-version": VERSION,
          "mcp-method": rpcRequest(raw)!.method,
          "x-omni-events-owner": owner,
          "x-omni-events-authorization": authorization,
          "user-agent": USER_AGENT,
        },
        body: new Uint8Array(raw),
        redirect: "error",
        signal: AbortSignal.timeout(30000),
      });
      const response: unknown = await upstream.json();
      if (!response || typeof response !== "object" || Array.isArray(response))
        throw new Error("Invalid Omni response");
      const rpc = response as Record<string, unknown>;
      if (rpc.result && typeof rpc.result === "object" && !Array.isArray(rpc.result)) {
        const body = rpc.result as Record<string, unknown>;
        json(res, upstream.status, {
          ...rpc,
          result: {
            resultType: "complete",
            ...body,
            _meta: {
              ...(body._meta as object | undefined),
              "io.modelcontextprotocol/serverInfo": SERVER_INFO,
            },
          },
        });
      } else json(res, upstream.status, rpc);
    },
    catch: () => new Error("Omni Events unavailable"),
  });
}

function legacyMethod(method: string): boolean {
  return [
    "tools/list",
    "resources/list",
    "resources/read",
    "resources/templates/list",
    "prompts/list",
    "prompts/get",
  ].includes(method);
}

export function makeHandler(options: AdapterOptions) {
  if (
    !options.allowedUserId ||
    !options.omniMcpToken ||
    !options.executorBaseUrl ||
    !options.omniBaseUrl ||
    !options.publicMcpOrigin
  )
    throw new Error("Missing adapter configuration");
  const continuations = new Continuations(options);
  return (req: IncomingMessage, res: ServerResponse) =>
    Effect.gen(function* () {
      const url = new URL(req.url ?? "/", "http://adapter.internal");
      if (url.pathname === "/health" && req.method === "GET") {
        json(res, 200, { status: "ok" });
        return;
      }
      if (url.pathname !== "/mcp") {
        json(res, 404, { error: "Not found" });
        return;
      }
      const raw = req.method === "POST" ? yield* readBody(req) : undefined;
      const message = raw ? rpcRequest(raw) : null;
      const envelope = message?.params?._meta;
      const metadata =
        envelope && typeof envelope === "object" && !Array.isArray(envelope)
          ? (envelope as Record<string, unknown>)
          : null;
      const headerVersion = req.headers["mcp-protocol-version"];
      const metadataVersion = metadata?.["io.modelcontextprotocol/protocolVersion"];
      const version = headerVersion ?? metadataVersion;
      const modern =
        version === VERSION ||
        (typeof version === "string" && version.startsWith("2026-")) ||
        message?.method === "server/discover";
      if (!modern) {
        yield* proxyLegacy(
          req,
          res,
          new URL(`/mcp${url.search}`, options.executorBaseUrl),
          raw,
        );
        return;
      }
      if (raw && raw.length > 262144) {
        error(res, message?.id ?? null, -32600, "Request too large", 413);
        return;
      }
      if (req.method !== "POST" || !message) {
        error(res, message?.id ?? null, -32600, "Invalid request", 400);
        return;
      }
      if (
        headerVersion !== undefined &&
        metadataVersion !== undefined &&
        headerVersion !== metadataVersion
      ) {
        error(res, message.id, -32020, "Protocol version header mismatch", 400);
        return;
      }
      if (version !== undefined && version !== VERSION) {
        error(res, message.id, -32022, "Unsupported protocol version", 400, {
          requested: version,
          supported: [VERSION],
        });
        return;
      }
      const authenticated = yield* Effect.result(
        authenticateOAuth(req.headers.authorization, options),
      );
      if (authenticated._tag === "Failure") {
        error(res, message.id, -32603, "Executor authentication unavailable", 503);
        return;
      }
      const identity = authenticated.success;
      if (!identity) {
        error(res, message.id, -32001, "Unauthorized", 401, undefined, {
          "www-authenticate": `Bearer resource_metadata="${new URL("/.well-known/oauth-protected-resource", options.publicMcpOrigin)}"`,
          "access-control-allow-origin": "*",
          "access-control-expose-headers": "WWW-Authenticate",
        });
        return;
      }
      if (message.method.startsWith("events/")) {
        if (
          !["events/list", "events/subscribe", "events/unsubscribe"].includes(
            message.method,
          )
        ) {
          error(res, message.id, -32601, "Method not found");
          return;
        }
        yield* forwardEvent(
          req,
          res,
          raw!,
          identity.owner,
          identity.authorization,
          options,
        );
        return;
      }
      if (message.method === "server/discover") {
        result(res, message.id, {
          supportedVersions: [VERSION],
          capabilities: { tools: {}, resources: {}, events: {} },
        });
        return;
      }
      if (message.method === "tools/call") {
        const clientCapabilities =
          metadata?.["io.modelcontextprotocol/clientCapabilities"];
        const elicitation =
          clientCapabilities &&
          typeof clientCapabilities === "object" &&
          !Array.isArray(clientCapabilities)
            ? (clientCapabilities as Record<string, unknown>).elicitation
            : undefined;
        const advertised =
          elicitation && typeof elicitation === "object" && !Array.isArray(elicitation)
            ? (elicitation as Record<string, unknown>)
            : null;
        const support = {
          form: advertised !== null && ("form" in advertised || !("url" in advertised)),
          url: advertised !== null && "url" in advertised,
        };
        const outcome = yield* Effect.result(
          continuations.call(
            identity.owner,
            identity.authorization,
            url.searchParams,
            message.params ?? {},
            support,
          ),
        );
        if (outcome._tag === "Failure") {
          const kind = outcome.failure.kind;
          error(
            res,
            message.id,
            kind === "upstream" ? -32603 : kind === "capacity" ? -32000 : -32602,
            kind === "upstream"
              ? "Executor tool request failed"
              : kind === "capacity"
                ? "Tool capacity exceeded"
                : "Tool continuation unavailable",
          );
        } else result(res, message.id, outcome.success as Record<string, unknown>);
        return;
      }
      if (!legacyMethod(message.method)) {
        error(res, message.id, -32601, "Method not found");
        return;
      }
      const outcome = yield* Effect.result(
        Effect.scoped(
          Effect.acquireRelease(
            openLegacyClient(options, identity.authorization, url.searchParams),
            (client) => Effect.promise(() => closeLegacyClient(client)),
          ).pipe(
            Effect.flatMap((client) =>
              callLegacy(client, message.method, message.params ?? {}),
            ),
          ),
        ),
      );
      if (outcome._tag === "Failure")
        error(res, message.id, -32603, "Executor tool request failed", 502);
      else if (outcome.success === undefined)
        error(res, message.id, -32601, "Method not found");
      else result(res, message.id, outcome.success as Record<string, unknown>);
    }).pipe(
      Effect.catch(() =>
        Effect.sync(() => {
          if (!res.headersSent) json(res, 502, { error: "Upstream unavailable" });
          else res.destroy();
        }),
      ),
    );
}

export function start(options: AdapterOptions, port = 4789) {
  const handler = makeHandler(options);
  return createServer((req, res) => {
    void Effect.runPromise(handler(req, res));
  }).listen(port, "0.0.0.0");
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  start(
    {
      executorBaseUrl: process.env.EXECUTOR_BASE_URL ?? "http://executor:4788",
      omniBaseUrl: process.env.OMNI_BASE_URL ?? "http://omni-notify:8080",
      omniMcpToken: process.env.OMNI_MCP_TOKEN ?? "",
      allowedUserId: process.env.EXECUTOR_ALLOWED_USER_ID ?? "",
      publicMcpOrigin: process.env.PUBLIC_MCP_ORIGIN ?? "",
    },
    Number(process.env.PORT ?? "4789"),
  );
}
