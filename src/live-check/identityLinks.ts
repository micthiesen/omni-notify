import type { Effect as EffectType } from "effect/Effect";
import { Entity } from "@micthiesen/mitools/entities";
import type { PersistenceError } from "../effect/errors.js";
import { fromSync } from "../effect/interop.js";
import { Platform } from "./platforms/index.js";
import type { PlatformBinding } from "./streamers.js";

export type ProfileIdentityLink = {
  /** Canonical platform account discovered from DGG, e.g. `kick:somebody`. */
  sourceBinding: string;
  /** Canonical configured account, resolved to its streamer through current config. */
  targetBinding: string;
  discoveredAt: number;
  verifiedAt: number;
};

export const ProfileIdentityLinkEntity = new Entity<
  ProfileIdentityLink,
  ["sourceBinding"]
>("live-profile-identity-link", ["sourceBinding"]);

/** Stable account key. Profile handles are case-insensitive on all supported hosts. */
export function canonicalBindingKey(binding: PlatformBinding): string {
  let username = binding.username.trim();
  if (binding.platform === Platform.YouTube) {
    if (username.startsWith("@")) {
      username = `@${username.slice(1).toLowerCase()}`;
    } else if (username.includes("/")) {
      const [kind, value] = username.split("/", 2);
      username = `${kind.toLowerCase()}/${value}`;
    }
  } else {
    username = username.toLowerCase();
  }
  return `${binding.platform}:${username}`;
}

export function getProfileIdentityLink(
  source: PlatformBinding,
): ProfileIdentityLink | undefined {
  return ProfileIdentityLinkEntity.get({
    sourceBinding: canonicalBindingKey(source),
  });
}

export function getProfileIdentityLinkEffect(
  source: PlatformBinding,
): EffectType<ProfileIdentityLink | undefined, PersistenceError> {
  return fromSync("read profile identity link", () => getProfileIdentityLink(source));
}

export function getAllProfileIdentityLinksEffect(): EffectType<
  ProfileIdentityLink[],
  PersistenceError
> {
  return fromSync("list profile identity links", () =>
    ProfileIdentityLinkEntity.getAll(),
  );
}

export function getProfileIdentityTarget(source: PlatformBinding): string | undefined {
  return getProfileIdentityLink(source)?.targetBinding;
}

export function forgetProfileIdentityLink(source: PlatformBinding): void {
  ProfileIdentityLinkEntity.delete({ sourceBinding: canonicalBindingKey(source) });
}

export function forgetProfileIdentityLinkEffect(
  source: PlatformBinding,
): EffectType<void, PersistenceError> {
  return fromSync("delete profile identity link", () =>
    forgetProfileIdentityLink(source),
  );
}

export function rememberProfileIdentityLink({
  source,
  target,
  now = Date.now(),
}: {
  source: PlatformBinding;
  target: PlatformBinding;
  now?: number;
}): ProfileIdentityLink {
  const sourceBinding = canonicalBindingKey(source);
  const targetBinding = canonicalBindingKey(target);
  const existing = ProfileIdentityLinkEntity.get({ sourceBinding });
  const row: ProfileIdentityLink = {
    sourceBinding,
    targetBinding,
    discoveredAt:
      existing?.targetBinding === targetBinding ? existing.discoveredAt : now,
    verifiedAt: now,
  };
  ProfileIdentityLinkEntity.upsert(row);
  return row;
}

export function rememberProfileIdentityLinkEffect(
  input: Parameters<typeof rememberProfileIdentityLink>[0],
): EffectType<ProfileIdentityLink, PersistenceError> {
  return fromSync("upsert profile identity link", () =>
    rememberProfileIdentityLink(input),
  );
}
