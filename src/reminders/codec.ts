import { deflateSync, inflateSync } from "node:zlib";

// Adapted from the MIT-licensed iobroker.icloud reminders implementation
// (07a91933e3f05a36d9c8918ece7f3de295aef805). See the upstream notice.
const MAX_TEXT_BYTES = 64 * 1024;
const REPLICA_UUID = Buffer.from("d46bcae41b8766c18d75efe35c9145c3", "hex");

function varint(value: number): Buffer {
  const bytes: number[] = [];
  let n = value >>> 0;
  do {
    bytes.push((n & 0x7f) | (n > 0x7f ? 0x80 : 0));
    n >>>= 7;
  } while (n);
  return Buffer.from(bytes);
}

function field(number: number, value: Buffer): Buffer {
  return Buffer.concat([varint((number << 3) | 2), varint(value.length), value]);
}

function integer(number: number, value: number): Buffer {
  return Buffer.concat([varint(number << 3), varint(value)]);
}

function charId(replica: number, clock: number): Buffer {
  return Buffer.concat([integer(1, replica), integer(2, clock)]);
}

/** Apple stores reminder text in a zlib-compressed versioned topotext document. */
export function encodeCrdtDocument(text: string): string {
  if (Buffer.byteLength(text, "utf8") > MAX_TEXT_BYTES) {
    throw new Error("Reminder text exceeds the size limit");
  }
  const length = text.length;
  const sentinel = Buffer.concat([
    field(1, charId(0, 0)),
    integer(2, 0),
    field(3, charId(0, 0)),
    integer(5, 1),
  ]);
  const content = Buffer.concat([
    field(1, charId(1, 0)),
    integer(2, length),
    field(3, charId(1, 0)),
    integer(5, 2),
  ]);
  const terminal = Buffer.concat([
    field(1, charId(0, 0xffff_ffff)),
    integer(2, 0),
    field(3, charId(0, 0xffff_ffff)),
  ]);
  const clock = field(
    1,
    Buffer.concat([
      field(1, REPLICA_UUID),
      field(2, integer(1, length)),
      field(2, integer(1, 1)),
    ]),
  );
  const string = Buffer.concat([
    field(2, Buffer.from(text, "utf8")),
    field(3, sentinel),
    ...(length ? [field(3, content)] : []),
    field(3, terminal),
    field(4, clock),
    ...(length ? [field(5, integer(1, length))] : []),
  ]);
  const version = Buffer.concat([integer(1, 0), integer(2, 0), field(3, string)]);
  return deflateSync(Buffer.concat([integer(1, 0), field(2, version)])).toString(
    "base64",
  );
}

function readVarint(data: Buffer, start: number): [number, number] | null {
  let value = 0;
  let offset = start;
  for (let shift = 0; shift <= 28 && offset < data.length; shift += 7) {
    const byte = data[offset++];
    value += (byte & 0x7f) * 2 ** shift;
    if (!(byte & 0x80)) return [value, offset];
  }
  return null;
}

function bytesAt(data: Buffer, target: number): Buffer[] | null {
  const result: Buffer[] = [];
  for (let offset = 0; offset < data.length;) {
    const tag = readVarint(data, offset);
    if (!tag) return null;
    offset = tag[1];
    const number = Math.floor(tag[0] / 8);
    const wire = tag[0] & 7;
    if (!number) return null;
    if (wire === 2) {
      const length = readVarint(data, offset);
      if (!length || length[0] > data.length - length[1]) return null;
      offset = length[1];
      if (number === target) result.push(data.subarray(offset, offset + length[0]));
      offset += length[0];
    } else if (wire === 0) {
      const value = readVarint(data, offset);
      if (!value) return null;
      offset = value[1];
    } else if (wire === 1 || wire === 5) {
      offset += wire === 1 ? 8 : 4;
      if (offset > data.length) return null;
    } else return null;
  }
  return result;
}

function documentText(data: Buffer): string | null {
  const versions = bytesAt(data, 2);
  if (!versions) return null;
  for (const version of versions) {
    const strings = bytesAt(version, 3);
    if (!strings) continue;
    for (const string of strings) {
      const values = bytesAt(string, 2);
      if (values?.length) return values[0].toString("utf8");
    }
  }
  // CloudKit has also returned bare Version and topotext.String documents.
  for (const string of bytesAt(data, 3) ?? []) {
    const values = bytesAt(string, 2);
    if (values?.length) return values[0].toString("utf8");
  }
  return null;
}

export function decodeCrdtDocument(value: string): string {
  if (value.length > MAX_TEXT_BYTES * 2 || !/^[A-Za-z0-9+/]*={0,2}$/.test(value)) {
    throw new Error("Invalid reminder document encoding");
  }
  const compressed = Buffer.from(value, "base64");
  if (!compressed.length || compressed.length > MAX_TEXT_BYTES) {
    throw new Error("Invalid reminder document size");
  }
  let data: Buffer;
  try {
    data = inflateSync(compressed, { maxOutputLength: MAX_TEXT_BYTES * 2 });
  } catch {
    throw new Error("Cannot decompress reminder document");
  }
  const text = documentText(data);
  if (text === null) throw new Error("Cannot decode reminder document");
  // Preserve the stored text exactly so write verification cannot change meaning.
  // JSON serialization and React rendering handle separators/control characters.
  return text;
}
