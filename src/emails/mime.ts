import MailComposer from "nodemailer/lib/mail-composer/index.js";
import { Data, Effect } from "effect";
import type { ComposedEmailParams } from "./send.js";
import { OUTGOING_EMAIL_FROM } from "./identity.js";

export class EmailMimeError extends Data.TaggedError("EmailMimeError")<{
  readonly cause: unknown;
}> {}

/** Preserve the same identity, body and date for SMTP and the private Sent copy. */
export function prepareComposedEmailEffect(
  params: ComposedEmailParams & { messageId: string; date: Date },
) {
  return Effect.tryPromise({
    try: async () => {
      const options = {
        from: OUTGOING_EMAIL_FROM,
        to: params.to,
        cc: params.cc,
        bcc: params.bcc,
        subject: params.subject,
        text: params.text,
        messageId: params.messageId,
        date: params.date,
        inReplyTo: params.inReplyTo,
        references: params.inReplyTo
          ? [...new Set([...(params.references ?? []), params.inReplyTo])]
          : params.references,
      };
      const wire = await new MailComposer(options).compile().build();
      const copy = new MailComposer(options).compile();
      copy.keepBcc = true;
      return {
        wire: wire.toString("base64"),
        content: (await copy.build()).toString("base64"),
        from: OUTGOING_EMAIL_FROM,
        date: params.date.toISOString(),
      };
    },
    catch: (cause) => new EmailMimeError({ cause }),
  });
}
