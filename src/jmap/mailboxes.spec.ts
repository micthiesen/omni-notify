import { describe, expect, it } from "vitest";
import { isEmailInAllowedMailbox, type MailboxRoles } from "./mailboxes.js";

const roles: MailboxRoles = new Map([
  ["mb-inbox", "inbox"],
  ["mb-archive", "archive"],
  ["mb-sent", "sent"],
  ["mb-drafts", "drafts"],
  ["mb-junk", "junk"],
  ["mb-trash", "trash"],
  ["mb-custom", null],
]);

describe("isEmailInAllowedMailbox", () => {
  it("allows emails in the inbox", () => {
    expect(isEmailInAllowedMailbox({ "mb-inbox": true }, roles)).toBe(true);
  });

  it("allows emails in the archive", () => {
    expect(isEmailInAllowedMailbox({ "mb-archive": true }, roles)).toBe(true);
  });

  it("drops sent, drafts, junk, and trash mail", () => {
    for (const id of ["mb-sent", "mb-drafts", "mb-junk", "mb-trash"]) {
      expect(isEmailInAllowedMailbox({ [id]: true }, roles)).toBe(false);
    }
  });

  it("drops mail filed only in a custom role-less folder", () => {
    expect(isEmailInAllowedMailbox({ "mb-custom": true }, roles)).toBe(false);
  });

  it("allows mail when any mailbox is inbox or archive", () => {
    expect(isEmailInAllowedMailbox({ "mb-junk": true, "mb-inbox": true }, roles)).toBe(
      true,
    );
    expect(
      isEmailInAllowedMailbox({ "mb-custom": true, "mb-archive": true }, roles),
    ).toBe(true);
  });

  it("ignores mailbox ids mapped to false", () => {
    expect(isEmailInAllowedMailbox({ "mb-inbox": false, "mb-sent": true }, roles)).toBe(
      false,
    );
  });

  it("fails open when role resolution failed", () => {
    expect(isEmailInAllowedMailbox({ "mb-sent": true }, undefined)).toBe(true);
  });

  it("fails open when the email has no mailbox info", () => {
    expect(isEmailInAllowedMailbox(undefined, roles)).toBe(true);
    expect(isEmailInAllowedMailbox({}, roles)).toBe(true);
  });

  it("fails open when every mailbox id is unknown to the snapshot", () => {
    expect(isEmailInAllowedMailbox({ "mb-new": true }, roles)).toBe(true);
  });

  it("drops mail when a known disallowed mailbox is present among unknowns", () => {
    expect(isEmailInAllowedMailbox({ "mb-new": true, "mb-sent": true }, roles)).toBe(
      false,
    );
  });
});
