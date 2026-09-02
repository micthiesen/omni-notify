import type { Effect as EffectType } from "effect/Effect";
import type { Docstore } from "@micthiesen/mitools/docstore";
import { Entity } from "@micthiesen/mitools/entities";
import { Effect, Option } from "effect";
import type { PersistenceError } from "../effect/errors.js";
import { PersistenceError as PersistenceFailure } from "../effect/errors.js";
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

export function getProfileIdentityLinkEffect(
  source: PlatformBinding,
): EffectType<ProfileIdentityLink | undefined, PersistenceError, Docstore> {
  return ProfileIdentityLinkEntity.get({
    sourceBinding: canonicalBindingKey(source),
  }).pipe(
    Effect.map(Option.getOrUndefined),
    Effect.mapError(
      (cause) =>
        new PersistenceFailure({ operation: "read profile identity link", cause }),
    ),
  );
}

export function getAllProfileIdentityLinksEffect(): EffectType<
  ProfileIdentityLink[],
  PersistenceError,
  Docstore
> {
  return ProfileIdentityLinkEntity.getAll().pipe(
    Effect.mapError(
      (cause) =>
        new PersistenceFailure({ operation: "list profile identity links", cause }),
    ),
  );
}

export function forgetProfileIdentityLinkEffect(
  source: PlatformBinding,
): EffectType<void, PersistenceError, Docstore> {
  return ProfileIdentityLinkEntity.delete({
    sourceBinding: canonicalBindingKey(source),
  }).pipe(
    Effect.asVoid,
    Effect.mapError(
      (cause) =>
        new PersistenceFailure({ operation: "delete profile identity link", cause }),
    ),
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
}): EffectType<ProfileIdentityLink, PersistenceError, Docstore> {
  const sourceBinding = canonicalBindingKey(source);
  const targetBinding = canonicalBindingKey(target);
  return Effect.gen(function* () {
    const existing = Option.getOrUndefined(
      yield* ProfileIdentityLinkEntity.get({ sourceBinding }),
    );
    const row: ProfileIdentityLink = {
      sourceBinding,
      targetBinding,
      discoveredAt:
        existing?.targetBinding === targetBinding ? existing.discoveredAt : now,
      verifiedAt: now,
    };
    yield* ProfileIdentityLinkEntity.upsert(row);
    return row;
  }).pipe(
    Effect.mapError(
      (cause) =>
        new PersistenceFailure({ operation: "upsert profile identity link", cause }),
    ),
  );
}

export function rememberProfileIdentityLinkEffect(
  input: Parameters<typeof rememberProfileIdentityLink>[0],
): EffectType<ProfileIdentityLink, PersistenceError, Docstore> {
  return rememberProfileIdentityLink(input);
}
