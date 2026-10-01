import type { ImapFlow } from "imapflow";
import { describe, expect, it, vi } from "vitest";
import { uidExpungeExact } from "./uidExpunge.js";

function fixture() {
  const commands = new Map<
    string,
    (client: unknown, uid: number) => Promise<boolean>
  >();
  const next = vi.fn();
  const exec = vi.fn(async () => ({ next }));
  const client = {
    capabilities: new Map([["UIDPLUS", true]]),
    commands,
    exec,
    run: vi.fn(async (name: string, uid: number) => commands.get(name)!(client, uid)),
  };
  return { client: client as unknown as ImapFlow, commands, exec, next };
}

describe("scoped UID EXPUNGE command", () => {
  it("runs through ImapFlow's IDLE-aware dispatcher and sends one UID", async () => {
    const { client, commands, exec, next } = fixture();
    expect(await uidExpungeExact(client, 37)).toBe(true);
    expect(exec).toHaveBeenCalledExactlyOnceWith("UID EXPUNGE", [
      { type: "SEQUENCE", value: "37" },
    ]);
    expect(next).toHaveBeenCalledOnce();
    expect(commands.size).toBe(0);
  });

  it("rejects missing UIDPLUS and cleans up a failed wire command", async () => {
    const { client, commands, exec } = fixture();
    client.capabilities.delete("UIDPLUS");
    await expect(uidExpungeExact(client, 37)).rejects.toThrow(/unavailable/);
    expect(exec).not.toHaveBeenCalled();
    client.capabilities.set("UIDPLUS", true);
    exec.mockRejectedValueOnce(new Error("connection lost"));
    await expect(uidExpungeExact(client, 37)).rejects.toThrow(/connection lost/);
    expect(commands.size).toBe(0);
  });
});
