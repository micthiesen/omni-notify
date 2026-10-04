import { Duration } from "effect";
import { Hono } from "hono";
import { describe, expect, it } from "vitest";
import { testRuntime } from "../live-check/testRuntime.js";
import { registerDeviceLinkRoutes } from "./routes.js";
import { DeviceLinkService } from "./service.js";

const token = "device-token-that-is-definitely-long-enough";

function setup() {
  const app = new Hono();
  const link = new DeviceLinkService();
  registerDeviceLinkRoutes(testRuntime, app, link, token);
  const post = (path: string, body: unknown, auth = token) =>
    app.request(path, {
      method: "POST",
      headers: { Authorization: `Bearer ${auth}`, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  return { app, link, post };
}

describe("device link routes", () => {
  it("rejects requests without the device token", async () => {
    const { post } = setup();
    const response = await post("/device-link/poll", { v: 1, disabled: true }, "wrong");
    expect(response.status).toBe(401);
    expect((await post("/device-link/result", { v: 1, id: "x" }, "")).status).toBe(401);
  });

  it("rejects malformed bodies", async () => {
    const { post } = setup();
    expect((await post("/device-link/poll", { v: 2, disabled: false })).status).toBe(
      400,
    );
    expect((await post("/device-link/result", { v: 1 })).status).toBe(400);
  });

  it("relays a job to the polling Mac and its result back to the caller", async () => {
    const { link, post } = setup();
    const polling = post("/device-link/poll", {
      v: 1,
      disabled: false,
      host: "MaxBook",
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    const call = testRuntime.runPromise(
      link.execute("projects", {}, Duration.seconds(5)),
    );
    const response = await polling;
    expect(response.status).toBe(200);
    const { jobs } = (await response.json()) as {
      jobs: { id: string; command: string; args: unknown }[];
    };
    expect(jobs).toEqual([{ id: expect.any(String), command: "projects", args: {} }]);

    const output = { v: 1, ok: true, data: { projects: [] } };
    const result = await post("/device-link/result", { v: 1, id: jobs[0]!.id, output });
    expect(await result.json()).toEqual({ v: 1, accepted: true });
    await expect(call).resolves.toEqual({ projects: [] });

    const again = await post("/device-link/result", { v: 1, id: jobs[0]!.id, output });
    expect(await again.json()).toEqual({ v: 1, accepted: false });
  });

  it("stops holding a poll when the Mac disconnects", async () => {
    const { app, link } = setup();
    const controller = new AbortController();
    const polling = app.request("/device-link/poll", {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({ v: 1, disabled: true }),
      signal: controller.signal,
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    controller.abort();
    const response = await polling;
    expect(await response.json()).toEqual({ v: 1, jobs: [] });
    const status = await testRuntime.runPromise(link.status());
    expect(status.disabled).toBe(true);
  });
});
