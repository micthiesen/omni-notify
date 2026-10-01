export interface RemindersConfiguration {
  enabled?: string;
  account?: string;
  password?: string;
  storageKey?: string;
  publicOrigin?: string;
  directory: string;
}

/** Invalid or incomplete configuration disables this feature, never the application. */
export function remindersConfigured(config: RemindersConfiguration): boolean {
  if (
    config.enabled !== "true" ||
    !config.account?.trim() ||
    !config.password ||
    !/^[a-f0-9]{64}$/i.test(config.storageKey ?? "")
  )
    return false;
  try {
    const url = new URL(config.publicOrigin ?? "");
    return (
      url.protocol === "https:" &&
      url.origin === config.publicOrigin &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}
