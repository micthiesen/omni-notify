import type { EmailHandler, FetchedEmail } from "../../email/types.js";

/** Preserve an existing scheduled retry unless the manual reprocess succeeds. */
export async function handleEmailThenClearRetry(
  handler: Pick<EmailHandler, "handleEmails">,
  email: FetchedEmail,
  clearRetry: () => void,
): Promise<void> {
  await handler.handleEmails([email]);
  clearRetry();
}
