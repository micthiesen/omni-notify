import { extractHttpError } from "@micthiesen/mitools/http";
import type { Logger } from "@micthiesen/mitools/logging";
import got from "got";
import * as mm from "music-metadata";
import NodeID3 from "node-id3";

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
 * Embed the article's lead image as ID3 album art. Best-effort: any failure
 * (unreachable image, bad mime) returns the audio untouched.
 */
export async function addAlbumArt(
  audioFile: Buffer,
  leadImageUrl: string | undefined,
  logger: Logger,
): Promise<Buffer> {
  if (!leadImageUrl) return audioFile;

  try {
    const imageResponse = await got.get(leadImageUrl, { responseType: "buffer" });
    const respMime = imageResponse.headers["content-type"];
    if (!respMime?.includes("image")) {
      throw new Error("No mime type found for image");
    }
    const tags = {
      image: {
        // Trust the response header; the URL path often carries query strings.
        mime: respMime.split(";")[0].trim(),
        type: { id: 3, name: "front cover" },
        description: "Cover",
        imageBuffer: Buffer.from(imageResponse.body),
      },
    };

    const tagged = await new Promise<Buffer>((resolve, reject) => {
      NodeID3.write(tags, audioFile, (err, buffer) => {
        if (err || !buffer) reject(err ?? new Error("NodeID3 returned no buffer"));
        else resolve(buffer);
      });
    });
    logger.info("Album art embedded in audio file");
    return tagged;
  } catch (error) {
    logger.warn("Error adding album art:", { error: extractHttpError(error) });
    return audioFile;
  }
}
