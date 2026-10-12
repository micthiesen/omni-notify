import CryptoKit
import Foundation
import ImageIO
import UniformTypeIdentifiers

/// The last good widget payload and when this device fetched it.
struct CachedSnapshot<Value: Codable & Sendable>: Codable, Sendable {
    let value: Value
    let fetchedAt: Date
}

/// Last-good snapshots in the App Group, so widgets survive being away from the LAN.
enum SnapshotStore {
    static func load<Value: Codable & Sendable>(_ name: String, as _: Value.Type) -> CachedSnapshot<Value>? {
        guard let data = try? Data(contentsOf: url(name)) else { return nil }
        return try? JSONDecoder().decode(CachedSnapshot<Value>.self, from: data)
    }

    static func save<Value: Codable & Sendable>(_ value: Value, name: String, at date: Date = Date()) {
        guard let data = try? JSONEncoder().encode(CachedSnapshot(value: value, fetchedAt: date)) else {
            return
        }
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try? data.write(to: url(name), options: .atomic)
    }

    /// Fresh data when the fetch works (cached for next time), else the last good copy
    /// while it is younger than `maxAge`. Older copies are not shown as current: the
    /// widget falls back to its "out of reach" state instead.
    static func resolve<Value: Codable & Sendable>(
        _ name: String,
        maxAge: TimeInterval,
        fetch: () async throws -> Value
    ) async -> (snapshot: CachedSnapshot<Value>?, error: Error?) {
        do {
            let value = try await fetch()
            save(value, name: name)
            return (CachedSnapshot(value: value, fetchedAt: Date()), nil)
        } catch {
            return (usable(load(name, as: Value.self), maxAge: maxAge, now: Date()), error)
        }
    }

    static func usable<Value>(_ cached: CachedSnapshot<Value>?, maxAge: TimeInterval, now: Date)
        -> CachedSnapshot<Value>?
    {
        guard let cached, now.timeIntervalSince(cached.fetchedAt) <= maxAge else { return nil }
        return cached
    }

    private static var directory: URL {
        OmniSettings.shared.sharedContainerURL.appending(path: "WidgetSnapshots", directoryHint: .isDirectory)
    }

    private static func url(_ name: String) -> URL {
        directory.appending(path: "\(name).json")
    }
}

/// An avatar ready to draw: a small downsampled bitmap plus its average color, which
/// widgets use for the warm glow behind a live hero (no runtime blur).
struct AvatarImage: @unchecked Sendable {
    let image: CGImage
    let red: Double
    let green: Double
    let blue: Double
}

/// Downloads, downsamples and caches streamer avatars in the App Group.
///
/// Widget extensions have a ~30 MB budget and the notification service extension a
/// ~24 MB one, so every image is fetched with a byte cap and decoded straight to a
/// thumbnail of at most `maxPixels` on the long edge (never the full-size bitmap).
enum AvatarStore {
    static let maxDownloadBytes = 2 * 1_024 * 1_024
    static let maxCachedFiles = 96
    static let maxAge: TimeInterval = 7 * 24 * 60 * 60
    /// The cached source is one thumbnail at the largest size anything draws (the XL
    /// hero at 3x); every smaller size is derived from it without another download.
    static let sourcePixels = 288

    static func avatar(
        for url: URL,
        maxPixels: Int,
        timeout: TimeInterval = 8
    ) async -> AvatarImage? {
        guard url.scheme?.lowercased() == "https" else { return nil }
        let file = cacheURL(for: url)
        let cached = try? Data(contentsOf: file)
        if let cached, isFresh(file), let image = decode(cached, maxPixels: maxPixels) {
            return image
        }
        guard let data = await CappedDownload.fetch(url, maxBytes: maxDownloadBytes, deadline: timeout),
              let source = thumbnailPNG(data, maxPixels: sourcePixels),
              let image = decode(source, maxPixels: maxPixels)
        else {
            // A stale cached copy beats a monogram when the CDN is unreachable.
            return cached.flatMap { decode($0, maxPixels: maxPixels) }
        }
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try? source.write(to: file, options: .atomic)
        prune()
        return image
    }

    private static func isFresh(_ file: URL) -> Bool {
        guard let modified = try? file.resourceValues(forKeys: [.contentModificationDateKey])
            .contentModificationDate
        else { return false }
        return Date().timeIntervalSince(modified) < maxAge
    }

    /// Fetches several avatars concurrently (at most three at a time).
    static func avatars(
        for requests: [(id: String, url: URL)],
        maxPixels: Int
    ) async -> [String: AvatarImage] {
        await withTaskGroup(of: (String, AvatarImage?).self) { group in
            var result: [String: AvatarImage] = [:]
            var iterator = requests.makeIterator()
            func addNext() {
                guard let next = iterator.next() else { return }
                group.addTask { (next.id, await avatar(for: next.url, maxPixels: maxPixels)) }
            }
            for _ in 0 ..< 3 { addNext() }
            while let (id, image) = await group.next() {
                if let image { result[id] = image }
                addNext()
            }
            return result
        }
    }

    /// Downsampled PNG bytes, also used by the notification service extension.
    static func thumbnailPNG(_ data: Data, maxPixels: Int) -> Data? {
        let options = [kCGImageSourceShouldCache: false] as CFDictionary
        guard let source = CGImageSourceCreateWithData(data as CFData, options) else { return nil }
        let thumbOptions = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixels,
        ] as CFDictionary
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, thumbOptions) else {
            return nil
        }
        return pngData(image)
    }

    static func pngData(_ image: CGImage) -> Data? {
        let output = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(
            output, UTType.png.identifier as CFString, 1, nil
        ) else { return nil }
        CGImageDestinationAddImage(destination, image, nil)
        return CGImageDestinationFinalize(destination) ? output as Data : nil
    }

    static func decode(_ data: Data, maxPixels: Int) -> AvatarImage? {
        guard let source = CGImageSourceCreateWithData(data as CFData, nil) else { return nil }
        let options = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixels,
        ] as CFDictionary
        guard let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options) else { return nil }
        let (r, g, b) = averageColor(of: image)
        return AvatarImage(image: image, red: r, green: g, blue: b)
    }

    /// Average color via a 1x1 redraw.
    static func averageColor(of image: CGImage) -> (Double, Double, Double) {
        var pixel = [UInt8](repeating: 0, count: 4)
        let drawn = pixel.withUnsafeMutableBytes { buffer -> Bool in
            guard let context = CGContext(
                data: buffer.baseAddress,
                width: 1,
                height: 1,
                bitsPerComponent: 8,
                bytesPerRow: 4,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ) else { return false }
            context.interpolationQuality = .medium
            context.draw(image, in: CGRect(x: 0, y: 0, width: 1, height: 1))
            return true
        }
        guard drawn, pixel[3] > 0 else { return (0.5, 0.5, 0.5) }
        let alpha = Double(pixel[3]) / 255
        return (
            Double(pixel[0]) / 255 / alpha,
            Double(pixel[1]) / 255 / alpha,
            Double(pixel[2]) / 255 / alpha
        )
    }

    private static var directory: URL {
        OmniSettings.shared.sharedContainerURL.appending(path: "Avatars", directoryHint: .isDirectory)
    }

    private static func cacheURL(for url: URL) -> URL {
        let hash = SHA256.hash(data: Data(url.absoluteString.utf8))
            .prefix(16)
            .map { String(format: "%02x", $0) }
            .joined()
        return directory.appending(path: "\(hash).png")
    }

    /// Keeps the newest `maxCachedFiles` and drops anything older than `maxAge` x 4.
    private static func prune() {
        let keys: [URLResourceKey] = [.contentModificationDateKey]
        guard let files = try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: keys
        ) else { return }
        let dated = files.map { file in
            (file, (try? file.resourceValues(forKeys: Set(keys)).contentModificationDate) ?? .distantPast)
        }.sorted { $0.1 > $1.1 }
        let cutoff = Date().addingTimeInterval(-maxAge * 4)
        for (index, entry) in dated.enumerated() where index >= maxCachedFiles || entry.1 < cutoff {
            try? FileManager.default.removeItem(at: entry.0)
        }
    }
}

/// One GET with a hard byte cap and an overall deadline (not just an idle timeout).
/// Data arrives in chunks through the delegate; the task is cancelled the moment the
/// body would pass `maxBytes`, so an oversized response never fills extension memory.
final class CappedDownload: NSObject, URLSessionDataDelegate, @unchecked Sendable {
    private let maxBytes: Int
    private let lock = NSLock()
    private var buffer = Data()
    private var exceeded = false
    private var continuation: CheckedContinuation<Data?, Never>?

    private init(maxBytes: Int) {
        self.maxBytes = maxBytes
    }

    static func fetch(_ url: URL, maxBytes: Int, deadline: TimeInterval) async -> Data? {
        let delegate = CappedDownload(maxBytes: maxBytes)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = deadline
        configuration.timeoutIntervalForResource = deadline
        configuration.urlCache = nil
        let session = URLSession(configuration: configuration, delegate: delegate, delegateQueue: nil)
        defer { session.finishTasksAndInvalidate() }
        return await withCheckedContinuation { continuation in
            delegate.lock.withLock { delegate.continuation = continuation }
            session.dataTask(with: url).resume()
        }
    }

    func urlSession(
        _: URLSession,
        dataTask _: URLSessionDataTask,
        didReceive response: URLResponse
    ) async -> URLSession.ResponseDisposition {
        guard let http = response as? HTTPURLResponse, 200 ..< 300 ~= http.statusCode,
              http.expectedContentLength <= Int64(maxBytes)
        else { return .cancel }
        return .allow
    }

    func urlSession(_: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
        let over = lock.withLock { () -> Bool in
            guard buffer.count + data.count <= maxBytes else {
                exceeded = true
                return true
            }
            buffer.append(data)
            return false
        }
        if over { dataTask.cancel() }
    }

    func urlSession(_: URLSession, task _: URLSessionTask, didCompleteWithError error: Error?) {
        let (continuation, result) = lock.withLock { () -> (CheckedContinuation<Data?, Never>?, Data?) in
            defer { self.continuation = nil }
            return (self.continuation, error == nil && !exceeded ? buffer : nil)
        }
        continuation?.resume(returning: result)
    }
}
