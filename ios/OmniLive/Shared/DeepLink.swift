import Foundation

/// `omni://open?u=<percent-encoded https URL>&a=<percent-encoded app URL>`.
///
/// Widgets whose whole surface is one tap target (small, circular, inline) can only
/// carry a `widgetURL`, which iOS always delivers to the containing app. The app
/// forwards it immediately without showing the web view: the app-scheme URL first
/// (e.g. `twitch://stream/x`) when present, then the https URL (a universal link
/// for Kick and YouTube). URLs on the Omni server load in the web view instead.
enum OmniDeepLink {
    static let scheme = "omni"
    static let openHost = "open"

    /// Strict unreserved set (RFC 3986), so `&`, `=`, `?`, `#`, `+` and `/` inside
    /// the embedded URL are always escaped.
    private static let unreserved = CharacterSet(
        charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"
    )

    static func openURL(target: URL, appURL: URL? = nil) -> URL {
        var query = "u=" + encode(target.absoluteString)
        if let appURL {
            query += "&a=" + encode(appURL.absoluteString)
        }
        // Built from escaped parts only, so this cannot fail.
        return URL(string: "\(scheme)://\(openHost)?\(query)")!
    }

    struct Open: Equatable, Sendable {
        let target: URL
        let appURL: URL?
    }

    static func parse(_ url: URL) -> Open? {
        guard url.scheme?.lowercased() == scheme,
              url.host()?.lowercased() == openHost,
              let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              let target = items.first(where: { $0.name == "u" })?.value.flatMap(webURL)
        else { return nil }
        let appURL = items.first(where: { $0.name == "a" })?.value.flatMap(URL.init(string:))
            .flatMap { isAllowedAppURL($0) ? $0 : nil }
        return Open(target: target, appURL: appURL)
    }

    /// Streaming apps only. A widget link must never be able to dial, text, or loop
    /// back into `omni://`.
    static let allowedAppSchemes: Set<String> = ["twitch", "youtube", "vnd.youtube", "kick"]

    static func isAllowedAppURL(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased() else { return false }
        return allowedAppSchemes.contains(scheme)
    }

    private static func encode(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: unreserved) ?? value
    }

    private static func webURL(_ raw: String) -> URL? {
        guard let url = URL(string: raw),
              let scheme = url.scheme?.lowercased(),
              ["http", "https"].contains(scheme),
              url.host() != nil
        else { return nil }
        return url
    }
}

/// Where a URL the app was asked to open should go.
enum OmniRoute: Equatable, Sendable {
    /// An Omni page: navigate the web view.
    case webView(URL)
    /// Somewhere else: hand to iOS, trying each candidate in order.
    case external([URL])

    static func decide(target: URL, appURL: URL?, server: URL?) -> OmniRoute {
        if let server, sameOrigin(target, server) {
            return .webView(target)
        }
        return .external([appURL, target].compactMap(\.self))
    }

    /// Same host is enough: `http://omni.boris` and `https://omni.boris/x` are both Omni.
    static func sameOrigin(_ a: URL, _ b: URL) -> Bool {
        guard let hostA = a.host()?.lowercased(), let hostB = b.host()?.lowercased() else {
            return false
        }
        return hostA == hostB
    }
}
