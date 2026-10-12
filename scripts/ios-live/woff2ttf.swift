// Rebuilds an sfnt (TTF/OTF) from the tables CoreText decodes out of a woff2 file.
// Usage: woff2ttf in.woff2 out.ttf (see convert-fonts.sh).
import Foundation
import CoreText
import CoreGraphics

let args = CommandLine.arguments
let data = try Data(contentsOf: URL(fileURLWithPath: args[1]))
guard let descs = CTFontManagerCreateFontDescriptorsFromData(data as CFData) as? [CTFontDescriptor], let d = descs.first else { fatalError("no descriptor") }
let font = CTFontCreateWithFontDescriptor(d, 12, nil)
print("name:", CTFontCopyPostScriptName(font))
guard let tags = CTFontCopyAvailableTables(font, CTFontTableOptions(rawValue: 0)) else { fatalError("no tables") }
var tables: [(UInt32, Data)] = []
for i in 0..<CFArrayGetCount(tags) {
    let raw = CFArrayGetValueAtIndex(tags, i)
    let tag = UInt32(UInt(bitPattern: raw))
    guard let t = CTFontCopyTable(font, CTFontTableTag(tag), CTFontTableOptions(rawValue: 0)) else { continue }
    tables.append((tag, t as Data))
}
tables.sort { $0.0 < $1.0 }
func be32(_ v: UInt32) -> Data { withUnsafeBytes(of: v.bigEndian) { Data($0) } }
func be16(_ v: UInt16) -> Data { withUnsafeBytes(of: v.bigEndian) { Data($0) } }
func checksum(_ d: Data) -> UInt32 {
    var p = d; while p.count % 4 != 0 { p.append(0) }
    var s: UInt32 = 0
    let bytes = [UInt8](p)
    for i in stride(from: 0, to: bytes.count, by: 4) {
        var w: UInt32 = UInt32(bytes[i]) << 24
        w |= UInt32(bytes[i + 1]) << 16
        w |= UInt32(bytes[i + 2]) << 8
        w |= UInt32(bytes[i + 3])
        s = s &+ w
    }
    return s
}
let n = UInt16(tables.count)
var es: UInt16 = 0; while (1 << (es + 1)) <= n { es += 1 }
let sr = UInt16(1 << es) * 16
var out = Data()
let hasCFF = tables.contains { $0.0 == 0x43464620 }
out += be32(hasCFF ? 0x4F54544F : 0x00010000)
out += be16(n) + be16(sr) + be16(es) + be16(n * 16 - sr)
var offset = UInt32(12 + 16 * tables.count)
var body = Data()
var headOffset: Int?
for (tag, var t) in tables {
    if tag == 0x68656164 { t.replaceSubrange(8..<12, with: Data(count: 4)); headOffset = Int(offset) }
    out += be32(tag) + be32(checksum(t)) + be32(offset) + be32(UInt32(t.count))
    var padded = t; while padded.count % 4 != 0 { padded.append(0) }
    body += padded; offset += UInt32(padded.count)
}
out += body
if let h = headOffset {
    let adj = 0xB1B0AFBA &- checksum(out)
    out.replaceSubrange((h + 8)..<(h + 12), with: be32(adj))
}
try out.write(to: URL(fileURLWithPath: args[2]))
print("tables:", tables.count, "bytes:", out.count)
