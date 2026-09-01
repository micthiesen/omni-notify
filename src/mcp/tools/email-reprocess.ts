import type { EmailHandler, FetchedEmail } from "../../email/types.js";
import { Data, Effect } from "effect";

export class EmailReprocessError extends Data.TaggedError("EmailReprocessError")<{
  readonly emailId: string;
  readonly cause: unknown;
}> {}

/** Preserve an existing scheduled retry unless the manual reprocess succeeds. */
export function handleEmailThenClearRetryEffect(
  handler: Pick<EmailHandler, "handleEmailsEffect">,
  email: FetchedEmail,
  clearRetry: () => void,
): Effect.Effect<void, EmailReprocessError> {
  return handler.handleEmailsEffect([email]).pipe(
    Effect.mapError((cause) => new EmailReprocessError({ emailId: email.id, cause })),
    Effect.andThen(Effect.sync(clearRetry)),
  );
}
