import type { Effect as EffectType } from "effect/Effect";
import type { EmailHandler, FetchedEmail } from "../../email/types.js";
import { Data, Effect } from "effect";

export class EmailReprocessError extends Data.TaggedError("EmailReprocessError")<{
  readonly emailId: string;
  readonly cause: unknown;
}> {}

/** Preserve an existing scheduled retry unless the manual reprocess succeeds. */
export function handleEmailThenClearRetryEffect<E>(
  handler: Pick<EmailHandler, "handleEmailsEffect">,
  email: FetchedEmail,
  clearRetry: () => EffectType<void, E>,
): EffectType<void, EmailReprocessError> {
  return handler.handleEmailsEffect([email]).pipe(
    Effect.andThen(clearRetry),
    Effect.mapError((cause) => new EmailReprocessError({ emailId: email.id, cause })),
  );
}
