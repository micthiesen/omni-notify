import type { Logger } from "@micthiesen/mitools/logging";
import { JamClient } from "jmap-jam";

const SESSION_URL = "https://api.fastmail.com/jmap/session";

export interface JmapContext {
  jam: JamClient;
  accountId: string;
}

export async function createJmapClient(
  bearerToken: string,
  logger: Logger,
): Promise<JmapContext> {
  const jam = new JamClient({ sessionUrl: SESSION_URL, bearerToken });

  const accountId = await jam.getPrimaryAccount();
  if (!accountId) {
    throw new Error("Could not resolve primary mail account from JMAP session");
  }

  logger.info(`JMAP session established (account: ${accountId})`);
  return { jam, accountId };
}
