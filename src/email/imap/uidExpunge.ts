import type { ImapFlow } from "imapflow";

type Reply = { next(): void };
type Internals = {
  capabilities: Map<string, boolean | number>;
  commands: Map<
    string,
    (connection: Internals, selectedUid: number) => Promise<boolean>
  >;
  run(name: string, selectedUid: number): Promise<boolean>;
  exec(
    name: string,
    attributes: Array<{ type: "SEQUENCE"; value: string }>,
  ): Promise<Reply>;
};

/** ImapFlow's EXPUNGE command also marks Deleted and may fall back to global
 * EXPUNGE. This temporary command uses its run() IDLE handshake and emits only
 * UID EXPUNGE for a single validated UID. */
export async function uidExpungeExact(client: ImapFlow, uid: number): Promise<boolean> {
  const command = "OMNI_UID_EXPUNGE_EXACT";
  const internal = client as unknown as Internals;
  if (
    !internal.capabilities.has("UIDPLUS") ||
    !Number.isSafeInteger(uid) ||
    uid < 1 ||
    internal.commands.has(command)
  )
    throw new Error("Exact UID EXPUNGE unavailable");
  internal.commands.set(command, async (connection, selectedUid) => {
    const reply = await connection.exec("UID EXPUNGE", [
      { type: "SEQUENCE", value: String(selectedUid) },
    ]);
    reply.next();
    return true;
  });
  try {
    return await internal.run(command, uid);
  } finally {
    internal.commands.delete(command);
  }
}
