import type { Logger } from "@micthiesen/mitools/logging";
import { Effect } from "effect";
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
export function downloadSupportedAttachmentsEffect(
  transport: EmailTransport,
  attachments: EmailAttachment[],
  logger: Logger,
): Effect.Effect<DownloadedAttachment[], never> {
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

  return Effect.forEach(
    supported,
    (attachment) =>
      transport.downloadAttachmentEffect(attachment).pipe(
        Effect.catch((cause) => {
          logger.warn(
            `Failed to download attachment "${attachment.name}": ${String(cause)}`,
          );
          return Effect.succeed(undefined);
        }),
        Effect.tap((downloaded) =>
          Effect.sync(() => {
            if (downloaded) {
              logger.debug(
                `Downloaded attachment "${downloaded.name}" (${downloaded.mimeType}, ` +
                  `${(downloaded.data.length / 1024).toFixed(0)}KB)`,
              );
            }
          }),
        ),
      ),
    { concurrency: 3 },
  ).pipe(Effect.map((results) => results.filter((item) => item !== undefined)));
}
