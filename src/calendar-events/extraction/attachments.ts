import type { Logger } from "@micthiesen/mitools/logging";
import type {
  DownloadedAttachment,
  EmailAttachment,
  EmailTransport,
} from "../../email/types.js";

const ALLOWED_MIME_TYPES = new Set(["application/pdf"]);

const MAX_ATTACHMENT_SIZE = 5 * 1024 * 1024; // 5 MB

export type { DownloadedAttachment };

/**
 * Download supported attachments (PDFs) via the email transport.
 * Skips unsupported types and oversized files.
 */
export async function downloadSupportedAttachments(
  transport: EmailTransport,
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
    // The transport logs its own failure reasons; a miss just skips the file.
    const downloaded = await transport.downloadAttachment(attachment);
    if (!downloaded) continue;
    results.push(downloaded);
    logger.debug(
      `Downloaded attachment "${downloaded.name}" (${downloaded.mimeType}, ` +
        `${(downloaded.data.length / 1024).toFixed(0)}KB)`,
    );
  }

  return results;
}
