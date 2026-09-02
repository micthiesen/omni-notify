import { extractHttpError } from "./httpError.js";
import type { NamedLogger as Logger } from "@micthiesen/mitools/logging";
import * as mm from "music-metadata";
import NodeID3 from "node-id3";
import { Effect, Schema } from "effect";
import type { Chapter } from "./types.js";
import { PressPodsError, tryPromise } from "./effect.js";
import { fetchPublicBuffer, PRESS_PODS_IMAGE_MAX_BYTES } from "./publicHttp.js";

export function getDuration(audioFile: Buffer, logger: Logger) {
  return tryPromise("parse episode audio metadata", () =>
    mm.parseBuffer(audioFile, undefined, { duration: true }),
  ).pipe(
    Effect.flatMap((metadata) =>
      Schema.decodeUnknownEffect(
        Schema.Struct({
          format: Schema.Struct({ duration: Schema.optional(Schema.Number) }),
        }),
      )(metadata),
    ),
    Effect.map((metadata) => metadata.format.duration),
    Effect.catch((error) =>
      logger
        .error("Error getting audio duration:", { error })
        .pipe(Effect.as(undefined)),
    ),
  );
}

/**
 * Embed ID3 metadata into the episode MP3: the article's lead image as album
 * art (fetched best-effort) plus chapter markers so podcast apps show a
 * scrubbable chapter list. Best-effort overall — any failure returns the audio
 * untouched rather than losing the episode over a tagging error.
 */
export function tagEpisodeAudio(
  audioFile: Buffer,
  {
    leadImageUrl,
    chapters,
    durationSeconds,
  }: {
    leadImageUrl?: string;
    chapters?: Chapter[];
    durationSeconds?: number;
  },
  logger: Logger,
) {
  return Effect.gen(function* () {
    const tags: NodeID3.Tags = {};

    if (leadImageUrl) {
      const imageResult = yield* fetchPublicBuffer(
        leadImageUrl,
        {
          headers: { Accept: "image/*" },
          timeout: { request: 20_000 },
          retry: { limit: 1, methods: ["GET"] },
        },
        "fetch PressPods album art",
        PRESS_PODS_IMAGE_MAX_BYTES,
      ).pipe(Effect.result);
      if (imageResult._tag === "Success") {
        const imageResponse = imageResult.success;
        const contentType = imageResponse.headers["content-type"];
        const respMime = Array.isArray(contentType) ? contentType[0] : contentType;
        if (!respMime?.includes("image")) {
          yield* logger.warn("Error fetching album art:", {
            error: "No image mime type",
          });
        } else
          tags.image = {
            // Trust the response header; the URL path often carries query strings.
            mime: respMime.split(";")[0].trim(),
            type: { id: 3, name: "front cover" },
            description: "Cover",
            imageBuffer: imageResponse.body,
          };
      } else {
        yield* logger.warn("Error fetching album art:", {
          error: extractHttpError(imageResult.failure.cause),
        });
      }
    }

    const chapterFrames = buildChapterFrames(chapters, durationSeconds);
    if (chapterFrames) {
      tags.chapter = chapterFrames.chapter;
      tags.tableOfContents = chapterFrames.tableOfContents;
    }

    if (Object.keys(tags).length === 0) return audioFile;

    const taggedResult = yield* Effect.callback<Buffer, PressPodsError>((resume) => {
      NodeID3.write(tags, audioFile, (err, buffer) => {
        if (err || !buffer) {
          resume(
            Effect.fail(
              new PressPodsError({
                operation: "write PressPods ID3 tags",
                cause: err ?? new Error("NodeID3 returned no buffer"),
              }),
            ),
          );
        } else resume(Effect.succeed(buffer));
      });
    }).pipe(Effect.result);
    if (taggedResult._tag === "Success") {
      const tagged = taggedResult.success;
      yield* logger.info("Embedded ID3 tags", {
        art: Boolean(tags.image),
        chapters: chapterFrames?.chapter.length ?? 0,
      });
      return tagged;
    } else {
      yield* logger.warn("Error writing ID3 tags:", {
        error: extractHttpError(taggedResult.failure.cause),
      });
      return audioFile;
    }
  });
}

/** Build ID3 CHAP frames + the CTOC that references them (needs ≥2 chapters). */
function buildChapterFrames(
  chapters: Chapter[] | undefined,
  durationSeconds: number | undefined,
):
  | {
      chapter: NonNullable<NodeID3.Tags["chapter"]>;
      tableOfContents: NonNullable<NodeID3.Tags["tableOfContents"]>;
    }
  | undefined {
  if (!chapters || chapters.length < 2) return undefined;
  const totalMs = durationSeconds ? Math.round(durationSeconds * 1000) : undefined;
  const chapter = chapters.map((c, i) => {
    const startTimeMs = Math.max(0, Math.round(c.startTimeSeconds * 1000));
    const next = chapters[i + 1];
    const endTimeMs = next
      ? Math.round(next.startTimeSeconds * 1000)
      : (totalMs ?? startTimeMs + 1000);
    return {
      elementID: `chp${i}`,
      startTimeMs,
      endTimeMs,
      tags: { title: c.title },
    };
  });
  return {
    chapter,
    tableOfContents: [
      {
        elementID: "toc",
        isOrdered: true,
        elements: chapter.map((c) => c.elementID),
      },
    ],
  };
}
