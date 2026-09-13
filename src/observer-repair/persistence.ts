import { Entity } from "@micthiesen/mitools/entities";
import { decodeDoc, Docstore } from "@micthiesen/mitools/docstore";
import { Data, Effect, Schema } from "effect";

export type ObserverRepairPhase =
  | "reserved"
  | "executing"
  | "repaired"
  | "unhandled"
  | "commented"
  | "resolved"
  | "done";

export type ObserverRepairState = {
  issueId: number;
  revision: string;
  phase: ObserverRepairPhase;
  outcome?: "repaired" | "unhandled";
  message?: string;
  notification: "pending" | "sending" | "sent";
  lease?: { owner: string; expiresAt: number };
};

export const ObserverRepairStateSchema = Schema.Struct({
  issueId: Schema.Number,
  revision: Schema.String,
  phase: Schema.Literals([
    "reserved",
    "executing",
    "repaired",
    "unhandled",
    "commented",
    "resolved",
    "done",
  ]),
  outcome: Schema.optional(Schema.Literals(["repaired", "unhandled"])),
  message: Schema.optional(Schema.String),
  notification: Schema.Literals(["pending", "sending", "sent"]),
  lease: Schema.optional(
    Schema.Struct({ owner: Schema.String, expiresAt: Schema.Number }),
  ),
});

export const ObserverRepairStateEntity = new Entity<ObserverRepairState, ["issueId"]>(
  "observer-repair-state",
  ["issueId"],
);

export const OBSERVER_REPAIR_LEASE_MS = 30 * 60 * 1_000;

export class ObserverRepairPersistenceError extends Data.TaggedError(
  "ObserverRepairPersistenceError",
)<{ readonly operation: string; readonly cause: unknown }> {
  public override get message(): string {
    return `${this.operation}: ${this.cause instanceof Error ? this.cause.message : String(this.cause)}`;
  }
}

const fail = (operation: string) => (cause: unknown) =>
  new ObserverRepairPersistenceError({ operation, cause });

const decodeState = (data: Buffer): ObserverRepairState =>
  Schema.decodeUnknownSync(ObserverRepairStateSchema)(decodeDoc(data));

export function acquireIssue(
  issueId: number,
  revision: string,
  owner: string,
  now: number,
) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const pk = ObserverRepairStateEntity.getPk({ issueId });
    return yield* docstore.transaction("acquire Observer repair lease", (tx) => {
      const raw = tx.getRawRow(pk, now);
      const current = raw ? decodeState(raw.data) : undefined;

      if (current?.revision === revision && current.phase === "done") return undefined;
      if (current?.revision !== revision && current && current.phase !== "done") {
        return undefined;
      }
      if (
        current?.phase !== "done" &&
        current?.lease &&
        current.lease.expiresAt > now
      ) {
        return undefined;
      }

      const resumed = current && current.phase === "executing";
      const next: ObserverRepairState =
        current?.phase === "done"
          ? { issueId, revision, phase: "reserved", notification: "pending" }
          : current
            ? {
                ...current,
                phase: resumed ? "unhandled" : current.phase,
                outcome: resumed ? "unhandled" : current.outcome,
                message: resumed
                  ? "Repair interrupted while executing; manual handling required"
                  : current.message,
              }
            : { issueId, revision, phase: "reserved", notification: "pending" };
      const leased = {
        ...next,
        lease: { owner, expiresAt: now + OBSERVER_REPAIR_LEASE_MS },
      };
      Schema.decodeUnknownSync(ObserverRepairStateSchema)(leased);
      tx.upsertDoc(pk, leased, { entity: ObserverRepairStateEntity.name }, now);
      return leased;
    });
  }).pipe(Effect.mapError(fail(`acquire Observer issue ${issueId}`)));
}

export function saveIssue(state: ObserverRepairState, owner: string, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const pk = ObserverRepairStateEntity.getPk({ issueId: state.issueId });
    yield* docstore.transaction("save Observer repair state", (tx) => {
      const raw = tx.getRawRow(pk, now);
      if (!raw) throw new Error(`No Observer repair state exists for ${state.issueId}`);
      const current = decodeState(raw.data);
      if (current.lease?.owner !== owner)
        throw new Error(`Observer repair lease is not owned by ${owner}`);
      if (current.lease.expiresAt <= now)
        throw new Error(`Observer repair lease owned by ${owner} has expired`);
      const next = { ...state, lease: current.lease };
      Schema.decodeUnknownSync(ObserverRepairStateSchema)(next);
      tx.upsertDoc(pk, next, { entity: ObserverRepairStateEntity.name }, now);
    });
  }).pipe(Effect.mapError(fail(`save Observer issue ${state.issueId}`)));
}

export function releaseIssue(issueId: number, owner: string, now: number) {
  return Effect.gen(function* () {
    const docstore = yield* Docstore;
    const pk = ObserverRepairStateEntity.getPk({ issueId });
    yield* docstore.transaction("release Observer repair lease", (tx) => {
      const raw = tx.getRawRow(pk, now);
      if (!raw) return;
      const current = decodeState(raw.data);
      if (current.lease?.owner !== owner) return;
      const { lease: _lease, ...released } = current;
      tx.upsertDoc(pk, released, { entity: ObserverRepairStateEntity.name }, now);
    });
  }).pipe(Effect.mapError(fail(`release Observer issue ${issueId}`)));
}

export function listPending() {
  return ObserverRepairStateEntity.getAll().pipe(
    Effect.flatMap((states) =>
      Schema.decodeUnknownEffect(Schema.Array(ObserverRepairStateSchema))(states),
    ),
    Effect.map((states) =>
      states.filter((state) => state.phase !== "done").slice(0, 100),
    ),
    Effect.mapError(fail("list pending Observer repairs")),
  );
}
