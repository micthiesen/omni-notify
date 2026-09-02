import type { NamedLogger } from "@micthiesen/mitools/logging";
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
export function downloadSupportedAttachmentsEffect<R>(
  transport: EmailTransport<unknown, R>,
  attachments: EmailAttachment[],
  logger: NamedLogger,
) {
  return Effect.gen(function* () {
    const supported: EmailAttachment[] = [];
    for (const attachment of attachments) {
      if (!ALLOWED_MIME_TYPES.has(attachment.type)) {
        yield* logger.debug(
          `Skipping attachment "${attachment.name}" (unsupported type: ${attachment.type})`,
        );
      } else if (attachment.size > MAX_ATTACHMENT_SIZE) {
        yield* logger.debug(
          `Skipping attachment "${attachment.name}" (too large: ${(attachment.size / 1024 / 1024).toFixed(1)}MB)`,
        );
      } else supported.push(attachment);
    }

    return yield* Effect.forEach(
      supported,
      (attachment) =>
        transport.downloadAttachmentEffect(attachment).pipe(
          Effect.catch((cause) =>
            logger
              .warn(
                `Failed to download attachment "${attachment.name}": ${String(cause)}`,
              )
              .pipe(Effect.as(undefined)),
          ),
          Effect.tap((downloaded) =>
            downloaded
              ? logger.debug(
                  `Downloaded attachment "${downloaded.name}" (${downloaded.mimeType}, ` +
                    `${(downloaded.data.length / 1024).toFixed(0)}KB)`,
                )
              : Effect.void,
          ),
        ),
      { concurrency: 3 },
    ).pipe(Effect.map((results) => results.filter((item) => item !== undefined)));
  });
}
