// Regenerates tests/golden/mime/*.json from the *.eml fixtures with the TS
// reference: mailparser's simpleParser plus src/email/imap/mapMessage.ts.
// Usage (repository root): npx tsx crates/omni-imap/scripts/golden-mime.mts
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { simpleParser, type AddressObject } from "mailparser";
import { mapParsedMessage } from "../../../src/email/imap/mapMessage.js";
import {
  attachmentPartId,
  declaredAttachmentMimeType,
} from "../../../src/email/imap/attachments.js";

const dir = new URL("../tests/golden/mime/", import.meta.url);

type Addr = { name: string; address?: string; group?: Addr[] };
const addrs = (value: AddressObject | AddressObject[] | undefined) =>
  value === undefined
    ? null
    : (Array.isArray(value) ? value : [value]).map((o) =>
        o.value.map(function plain(a): Addr {
          return a.group
            ? { name: a.name, group: a.group.map(plain) }
            : { name: a.name, address: a.address ?? "" };
        }),
      );

for (const file of readdirSync(dir).filter((f) => f.endsWith(".eml")).sort()) {
  const source = readFileSync(new URL(file, dir));
  const parsed = await simpleParser(source);
  const internalDate = new Date("2026-09-30T12:00:00.000Z");
  const mapped = mapParsedMessage(
    parsed,
    { folder: "INBOX", uidValidity: "1", uid: 7 },
    internalDate,
  );
  const html = typeof parsed.html === "string" ? parsed.html : null;
  const expected = {
    messageId: parsed.messageId ?? null,
    inReplyTo: parsed.inReplyTo ?? null,
    references:
      parsed.references === undefined
        ? null
        : Array.isArray(parsed.references)
          ? parsed.references
          : [parsed.references],
    subject: parsed.subject ?? null,
    date: parsed.date ? parsed.date.toISOString() : null,
    dateIsNow: parsed.date ? Math.abs(parsed.date.getTime() - Date.now()) < 60_000 : false,
    from: addrs(parsed.from),
    to: addrs(parsed.to),
    cc: addrs(parsed.cc),
    bcc: addrs(parsed.bcc),
    replyTo: addrs(parsed.replyTo),
    html,
    // Plain-text bodies only: HTML-derived text belongs to the enricher.
    text: html === null ? (parsed.text ?? null) : null,
    headerKeys: parsed.headerLines.map((h) => h.key),
    attachments: parsed.attachments.map((a) => ({
      rawPartId: (Reflect.get(a, "partId") as string | null) ?? null,
      partId: attachmentPartId(a) ?? null,
      contentType: a.contentType,
      declaredType: declaredAttachmentMimeType(a),
      filename: a.filename ?? null,
      contentDisposition: a.contentDisposition ?? null,
      contentId: a.contentId ?? null,
      cid: a.cid ?? null,
      related: a.related ?? false,
      size: a.size,
      sha256: createHash("sha256").update(a.content).digest("hex"),
    })),
    mapped: {
      ...mapped,
      textBody: html === null ? mapped.textBody : undefined,
      links: undefined,
      linkMetadata: undefined,
    },
  };
  writeFileSync(
    new URL(file.replace(/\.eml$/, ".json"), dir),
    `${JSON.stringify(expected, null, 2)}\n`,
  );
}
