import type { Logger } from "@micthiesen/mitools/logging";
import type { JmapContext } from "../../jmap/client.js";
import type { EmailAttachment } from "../../jmap/emailFetcher.js";

const ALLOWED_MIME_TYPES = new Set(["application/pdf"]);

const MAX_ATTACHMENT_SIZE = 5 * 1024 * 1024; // 5 MB

export interface DownloadedAttachment {
  name: string;
  mimeType: string;
  data: Buffer;
}

/**
 * Download supported attachments (PDFs and images) from JMAP.
 * Skips unsupported types and oversized files.
 */
export async function downloadSupportedAttachments(
  ctx: JmapContext,
  attachments: EmailAttachment[],
  logger: Logger,
): Promise<DownloadedAttachment[]> {
  const supported = attachments.filter((a) => {
    if (!ALLOWED_MIME_TYPES.has(a.type)) {
      logger.debug(`Skipping attachment "${a.name}" (unsupported type: ${a.type})`);
      return false;
    }
    if (a.size > MAX_ATTACHMENT_SIZE) {
      logger.debug(
        `Skipping attachment "${a.name}" (too large: ${(a.size / 1024 / 1024).toFixed(1)}MB)`,
      );
      return false;
    }
    return true;
  });

  if (supported.length === 0) return [];

  const results: DownloadedAttachment[] = [];
  for (const attachment of supported) {
    try {
      const response = await ctx.jam.downloadBlob({
        accountId: ctx.accountId,
        blobId: attachment.blobId,
        mimeType: attachment.type,
        fileName: attachment.name,
      });

      if (!response.ok) {
        logger.warn(
          `Failed to download "${attachment.name}": ${response.status} ${response.statusText}`,
        );
        continue;
      }

      const buffer = Buffer.from(await response.arrayBuffer());
      results.push({
        name: attachment.name,
        mimeType: attachment.type,
        data: buffer,
      });
      logger.debug(
        `Downloaded attachment "${attachment.name}" (${attachment.type}, ${(buffer.length / 1024).toFixed(0)}KB)`,
      );
    } catch (error) {
      logger.warn(`Error downloading "${attachment.name}"`, (error as Error).message);
    }
  }

  return results;
}
