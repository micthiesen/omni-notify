import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { Effect } from "effect";
import {
  acquireState,
  RECOVERY_LEASE_MS,
  RecoveryStateEntity,
  releaseState,
  saveState,
} from "./persistence.js";

const NOW = 1_800_000_000_000;

layer(Docstore.layerMemory)("Arr recovery persistence", (it) => {
  it.effect("acquires only one lease for concurrent workers of the same kind", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      const states = yield* Effect.all(
        Array.from({ length: 20 }, (_, index) =>
          acquireState("sonarr", `worker-${index}`, NOW),
        ),
        { concurrency: "unbounded" },
      );

      expect(states.filter((state) => state !== undefined)).toHaveLength(1);
    }),
  );

  it.effect("leases Sonarr and Radarr independently", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      const [sonarr, radarr] = yield* Effect.all(
        [
          acquireState("sonarr", "sonarr-worker", NOW),
          acquireState("radarr", "radarr-worker", NOW),
        ],
        { concurrency: "unbounded" },
      );

      expect(sonarr?.lease?.owner).toBe("sonarr-worker");
      expect(radarr?.lease?.owner).toBe("radarr-worker");
    }),
  );

  it.effect("rejects a save from a worker that does not own the lease", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      const state = yield* acquireState("sonarr", "owner", NOW);
      expect(state).toBeDefined();

      const result = yield* Effect.result(
        saveState(
          {
            ...state!,
            observations: {
              bad: {
                fingerprint: "bad",
                firstSeenAt: NOW,
                lastSeenAt: NOW,
                observations: 1,
              },
            },
          },
          "intruder",
          NOW + 1,
        ),
      );
      expect(result._tag).toBe("Failure");

      const blocked = yield* acquireState("sonarr", "another-worker", NOW + 2);
      expect(blocked).toBeUndefined();

      yield* releaseState("sonarr", "owner");
      const unchanged = yield* acquireState("sonarr", "another-worker", NOW + 2);
      expect(unchanged?.observations).toEqual({});
    }),
  );

  it.effect("fails closed when persisted nested action data is malformed", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      const docstore = yield* Docstore;
      yield* docstore.upsertDoc(
        RecoveryStateEntity.getPk({ kind: "radarr" }),
        {
          kind: "radarr",
          observations: {},
          actions: [
            {
              downloadId: "download-1",
              title: "Movie",
              target: {
                id: "not-a-number",
                title: "Movie",
                year: 2026,
                monitored: true,
                hasFile: false,
                path: "/movies/Movie",
                episodeIds: [],
                episodes: [],
                alternateTitles: [],
              },
              files: [],
              outputPath: "/downloads/Movie",
              decision: { action: "import", reason: "complete", source: "rules" },
              phase: "reserved",
              createdAt: NOW,
              updatedAt: NOW,
              notification: "pending",
            },
          ],
        },
        { entity: RecoveryStateEntity.name },
      );

      const result = yield* Effect.result(acquireState("radarr", "worker", NOW));
      expect(result._tag).toBe("Failure");
    }),
  );

  it.effect("does not let an expired owner release a newer worker's lease", () =>
    Effect.gen(function* () {
      yield* RecoveryStateEntity.deleteAll();
      yield* acquireState("sonarr", "old-owner", NOW);
      const afterExpiry = NOW + RECOVERY_LEASE_MS;
      const current = yield* acquireState("sonarr", "new-owner", afterExpiry);
      expect(current?.lease?.owner).toBe("new-owner");

      yield* releaseState("sonarr", "old-owner");
      expect(
        yield* acquireState("sonarr", "third-worker", afterExpiry + 1),
      ).toBeUndefined();
    }),
  );
});
