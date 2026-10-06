import { Docstore } from "@micthiesen/mitools/docstore";
import { expect, layer } from "@effect/vitest";
import { describe, it } from "vitest";
import { Effect } from "effect";
import { TestClock } from "effect/testing";
import {
  DeviceLinkError,
  type DeviceCommand,
  type DeviceLinkService,
} from "../../device-link/service.js";
import {
  ClaudeSessionWatchEntity,
  ClaudeSessionWatcher,
  finishedTurn,
  isSettled,
} from "./claudeSessions.js";
import {
  EventDeliveryEntity,
  EventPersistence,
  EventReceiptEntity,
  EventSubscriptionEntity,
} from "./persistence.js";
import { McpEventService } from "./service.js";

const secret = `whsec_${Buffer.alloc(32, 7).toString("base64")}`;
const SESSION = "25ffb449-1111-4222-8333-444455556666";

type Raw = Record<string, unknown>;

/** A host whose session list and per-session status the test controls. */
interface Host {
  sessions: Raw[];
  /** Sessions only a direct status lookup finds, such as stopped ones. */
  stopped: Raw[];
  online: boolean;
  calls: DeviceCommand[];
}

function fakeLink(state: Host) {
  return {
    status: () =>
      Effect.succeed({
        configured: true as const,
        online: state.online,
        disabled: false,
        host: null,
        lastSeenAt: null,
        pendingJobs: 0,
      }),
    execute: (command: DeviceCommand, args: Raw) => {
      state.calls.push(command);
      if (command === "list") return Effect.succeed({ sessions: state.sessions });
      const found = [...state.sessions, ...state.stopped].find(
        (row) => row.session_id === args.session,
      );
      return found
        ? Effect.succeed(found)
        : Effect.fail(
            new DeviceLinkError({
              code: "not_found",
              detail: "gone",
              retryable: false,
            }),
          );
    },
  } as unknown as DeviceLinkService;
}

const summary = (overrides: Raw = {}): Raw => ({
  id: "25ffb449",
  session_id: SESSION,
  project: "omni-notify",
  status: "idle",
  state: "idle",
  revision: 4,
  ...overrides,
});

describe("claude turn detection", () => {
  it("treats busy or working sessions as unsettled", () => {
    expect(isSettled({ status: "busy", state: "working" })).toBe(false);
    expect(isSettled({ status: "idle", state: "working" })).toBe(false);
    expect(isSettled({ status: "idle", state: "idle" })).toBe(true);
    expect(isSettled({ status: "stopped", state: "working" })).toBe(true);
  });

  it("needs a baseline and a settled session that moved on", () => {
    const settled = { status: "idle", state: "idle", revision: 5 };
    expect(finishedTurn(undefined, settled)).toBe(false);
    expect(finishedTurn({ revision: 3, settled: false }, settled)).toBe(true);
    expect(finishedTurn({ revision: 3, settled: true }, settled)).toBe(true);
    expect(finishedTurn({ revision: 5, settled: true }, settled)).toBe(false);
    expect(
      finishedTurn({ revision: 3, settled: false }, { ...settled, status: "busy" }),
    ).toBe(false);
  });
});

layer(Docstore.layerMemory)("Claude session events", (it) => {
  const setup = (project?: string) =>
    Effect.gen(function* () {
      yield* EventSubscriptionEntity.deleteAll();
      yield* EventReceiptEntity.deleteAll();
      yield* EventDeliveryEntity.deleteAll();
      yield* ClaudeSessionWatchEntity.deleteAll();
      yield* TestClock.adjust("1 hour");
      const events = yield* McpEventService.make("test-omni-bearer", undefined, {
        verify: () => Effect.void,
        deliver: () => Effect.succeed({ status: 204 }),
      });
      const host: Host = { sessions: [], stopped: [], online: true, calls: [] };
      const watcher = new ClaudeSessionWatcher(events, fakeLink(host));
      return {
        events,
        host,
        watcher,
        subscribe: () =>
          events.subscribe({
            name: "claude.session.turn_finished",
            arguments: project ? { project } : {},
            delivery: {
              mode: "webhook",
              url: "https://chatgpt.example.com/events/claude",
              secret,
            },
          }),
      };
    });

  it.effect("stays idle without a subscription or while the host is offline", () =>
    Effect.gen(function* () {
      const { host, watcher, subscribe } = yield* setup();
      yield* watcher.poll();
      expect(host.calls).toEqual([]);
      yield* subscribe();
      host.online = false;
      yield* watcher.poll();
      expect(host.calls).toEqual([]);
    }),
  );

  it.effect("publishes each finished turn once, after a baseline", () =>
    Effect.gen(function* () {
      const { host, watcher, subscribe } = yield* setup();
      yield* subscribe();
      host.sessions = [summary({ revision: 2 })];
      yield* watcher.poll();
      expect(yield* EventPersistence.deliveries()).toHaveLength(0);

      host.sessions = [summary({ status: "busy", state: "working", revision: 3 })];
      yield* watcher.poll();
      host.sessions = [summary({ revision: 6 })];
      yield* watcher.poll();
      yield* watcher.poll();
      const deliveries = yield* EventPersistence.deliveries();
      expect(deliveries).toHaveLength(1);
      expect(deliveries[0]).toMatchObject({
        name: "claude.session.turn_finished",
        data: {
          sessionId: SESSION,
          id: "25ffb449",
          project: "omni-notify",
          status: "idle",
          revision: 6,
        },
      });
    }),
  );

  it.effect("catches a turn Omni started that ended before the next poll", () =>
    Effect.gen(function* () {
      const { host, watcher, subscribe } = yield* setup("omni-notify");
      yield* subscribe();
      yield* watcher.noteTurnStarted({
        sessionId: SESSION,
        id: "25ffb449",
        project: null,
        revision: 4,
      });
      host.sessions = [summary({ revision: 7 })];
      yield* watcher.poll();
      expect(yield* EventPersistence.deliveries()).toHaveLength(1);
    }),
  );

  it.effect("reads a mid-turn session that stopped and left the running list", () =>
    Effect.gen(function* () {
      const { host, watcher, subscribe } = yield* setup();
      yield* subscribe();
      host.sessions = [summary({ status: "busy", state: "working", revision: 3 })];
      yield* watcher.poll();
      host.sessions = [];
      host.stopped = [summary({ status: "stopped", project: undefined, revision: 5 })];
      yield* watcher.poll();
      expect(host.calls).toContain("status");
      expect(yield* EventPersistence.deliveries()).toMatchObject([
        { data: { status: "stopped", project: "omni-notify", revision: 5 } },
      ]);
    }),
  );

  it.effect("ignores turns from a different project", () =>
    Effect.gen(function* () {
      const { host, watcher, subscribe } = yield* setup("dotfiles");
      yield* subscribe();
      host.sessions = [summary({ status: "busy", state: "working", revision: 3 })];
      yield* watcher.poll();
      host.sessions = [summary({ revision: 6 })];
      yield* watcher.poll();
      expect(yield* EventPersistence.deliveries()).toHaveLength(0);
    }),
  );
  it.effect("waits for a new revision after a turn Omni started", () =>
    Effect.gen(function* () {
      const { host, watcher, subscribe } = yield* setup();
      yield* subscribe();
      yield* watcher.noteTurnStarted({
        sessionId: SESSION,
        id: "25ffb449",
        project: null,
        revision: 4,
      });
      // A resumed session can report idle before its turn begins.
      host.sessions = [summary({ revision: 4 })];
      yield* watcher.poll();
      expect(yield* EventPersistence.deliveries()).toHaveLength(0);
      host.sessions = [summary({ revision: 6 })];
      yield* watcher.poll();
      expect(yield* EventPersistence.deliveries()).toHaveLength(1);
    }),
  );

  it.effect("forgets sessions the host cannot read", () =>
    Effect.gen(function* () {
      const { watcher, subscribe } = yield* setup();
      yield* subscribe();
      const note = (sessionId: string) =>
        watcher.noteTurnStarted({ sessionId, id: null, project: null, revision: 1 });
      yield* note("gone-session");
      yield* watcher.poll();
      expect(yield* ClaudeSessionWatchEntity.getAll()).toHaveLength(0);
    }),
  );
});
