import { Logger } from "@micthiesen/mitools/logging";
import { describe, expect, it } from "@effect/vitest";
import { Effect } from "effect";
import { vi } from "vitest";
import type {
  LimitedTextResponse,
  PublicTextRequest,
} from "../../effect/publicHttp.js";
import {
  getCarrierCodesForPromptEffect,
  getValidCarrierCodesEffect,
} from "./carrierMap.js";

function response(
  chunks: string[],
  contentLength?: number,
  destroy = vi.fn(),
): LimitedTextResponse {
  return {
    response: {
      headers:
        contentLength === undefined ? {} : { "content-length": String(contentLength) },
    },
    destroy,
    async *[Symbol.asyncIterator]() {
      for (const chunk of chunks) yield chunk;
    },
  };
}

describe("carrier map bounded Effect cache", () => {
  it.effect("rejects fixed-length overflow before buffering", () =>
    Effect.gen(function* () {
      const destroy = vi.fn();
      const request = vi.fn(() => response([], 9, destroy)) as PublicTextRequest;
      const logger = new Logger("CarrierMapEffectSpec");

      const prompt = yield* getCarrierCodesForPromptEffect(logger, {
        request,
        maxResponseBytes: 8,
      });

      expect(prompt).toBe("");
      expect(destroy).toHaveBeenCalledOnce();
    }),
  );

  it.effect("cancels chunked overflow", () =>
    Effect.gen(function* () {
      const destroy = vi.fn();
      const request = vi.fn(() =>
        response(["12345", "67890"], undefined, destroy),
      ) as PublicTextRequest;
      const logger = new Logger("CarrierMapEffectSpec");

      const prompt = yield* getCarrierCodesForPromptEffect(logger, {
        request,
        maxResponseBytes: 8,
      });

      expect(prompt).toBe("");
      expect(destroy).toHaveBeenCalledOnce();
    }),
  );

  it.effect("coalesces concurrent first successful refreshes", () =>
    Effect.gen(function* () {
      const request = vi.fn(() =>
        response([
          JSON.stringify({
            canadapost: "Canada Post",
            ups: { name: "UPS" },
          }),
        ]),
      ) as PublicTextRequest;
      const logger = new Logger("CarrierMapEffectSpec");
      const dependencies = { request, maxResponseBytes: 1024 };

      const [prompt, codes] = yield* Effect.all(
        [
          getCarrierCodesForPromptEffect(logger, dependencies),
          getValidCarrierCodesEffect(logger, dependencies),
          getCarrierCodesForPromptEffect(logger, dependencies),
        ],
        { concurrency: "unbounded" },
      );

      expect(request).toHaveBeenCalledTimes(1);
      expect(prompt).toContain("canadapost: Canada Post");
      expect(codes).toContain("ups");
    }),
  );
});
