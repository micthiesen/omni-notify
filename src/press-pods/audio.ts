import { extractHttpError } from "@micthiesen/mitools/http";
import type { Logger } from "@micthiesen/mitools/logging";
import got from "got";
import * as mm from "music-metadata";
import NodeID3 from "node-id3";
import type { Chapter } from "./types.js";

export async function getDuration(
  audioFile: Buffer,
  logger: Logger,
): Promise<number | undefined> {
  try {
    const metadata = await mm.parseBuffer(audioFile, undefined, { duration: true });
    return metadata.format.duration;
  } catch (error) {
    logger.error("Error getting audio duration:", { error });
    return undefined;
  }
}

/**
 * Embed ID3 metadata into the episode MP3: the article's lead image as album
 * art (fetched best-effort) plus chapter markers so podcast apps show a
 * scrubbable chapter list. Best-effort overall — any failure returns the audio
 * untouched rather than losing the episode over a tagging error.
 */
export async function tagEpisodeAudio(
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
): Promise<Buffer> {
  const tags: NodeID3.Tags = {};

  if (leadImageUrl) {
    try {
      const imageResponse = await got.get(leadImageUrl, { responseType: "buffer" });
      const respMime = imageResponse.headers["content-type"];
      if (!respMime?.includes("image")) throw new Error("No mime type found for image");
      tags.image = {
        // Trust the response header; the URL path often carries query strings.
        mime: respMime.split(";")[0].trim(),
        type: { id: 3, name: "front cover" },
        description: "Cover",
        imageBuffer: Buffer.from(imageResponse.body),
      };
    } catch (error) {
      logger.warn("Error fetching album art:", { error: extractHttpError(error) });
    }
  }

  const chapterFrames = buildChapterFrames(chapters, durationSeconds);
  if (chapterFrames) {
    tags.chapter = chapterFrames.chapter;
    tags.tableOfContents = chapterFrames.tableOfContents;
  }

  if (Object.keys(tags).length === 0) return audioFile;

  try {
    const tagged = await new Promise<Buffer>((resolve, reject) => {
      NodeID3.write(tags, audioFile, (err, buffer) => {
        if (err || !buffer) reject(err ?? new Error("NodeID3 returned no buffer"));
        else resolve(buffer);
      });
    });
    logger.info("Embedded ID3 tags", {
      art: Boolean(tags.image),
      chapters: chapterFrames?.chapter.length ?? 0,
    });
    return tagged;
  } catch (error) {
    logger.warn("Error writing ID3 tags:", { error: extractHttpError(error) });
    return audioFile;
  }
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
