import Foundation

/// The `omni` dictionary of an alert push (see the iOS notification contract).
struct OmniPushPayload: Codable, Equatable, Sendable {
    struct Sender: Codable, Equatable, Sendable {
        let id: String
        let name: String
        let avatarUrl: String?

        var avatarURL: URL? {
            guard let avatarUrl, let url = URL(string: avatarUrl),
                  url.scheme?.lowercased() == "https"
            else { return nil }
            return url
        }
    }

    let channel: String?
    let url: String?
    let urlTitle: String?
    let sender: Sender?
    let refreshWidgets: [String]?

    /// A tappable web URL, if the push carries one.
    var targetURL: URL? {
        guard let url, let parsed = URL(string: url),
              let scheme = parsed.scheme?.lowercased(),
              ["http", "https"].contains(scheme)
        else { return nil }
        return parsed
    }

    /// Widget kinds to reload, deduplicated, unknown names dropped.
    var widgetKindsToReload: [String] {
        var seen = Set<String>()
        return (refreshWidgets ?? []).compactMap(OmniWidgetKind.kind(forServerName:))
            .filter { seen.insert($0).inserted }
    }

    /// Reads `userInfo["omni"]`. Malformed or missing data yields nil, never a crash.
    static func parse(userInfo: [AnyHashable: Any]) -> OmniPushPayload? {
        guard let omni = userInfo["omni"] as? [String: Any],
              JSONSerialization.isValidJSONObject(omni),
              let data = try? JSONSerialization.data(withJSONObject: omni)
        else { return nil }
        return try? JSONDecoder().decode(OmniPushPayload.self, from: data)
    }
}

/// Notification categories registered by the app at launch.
///
/// Action titles cannot change per notification, and categories registered from the
/// service extension are not guaranteed to apply before that notification shows. So the
/// app registers a fixed set up front: one generic `OMNI_LINK` ("Open") plus a few with
/// exact titles the server uses, and the service extension only picks among them.
enum OmniNotificationCategory {
    static let link = "OMNI_LINK"
    static let plain = "OMNI_PLAIN"
    static let openAction = "OMNI_OPEN"

    /// Registered link categories: identifier and action title.
    static let linkVariants: [(identifier: String, title: String)] = [
        (link, "Open"),
        ("OMNI_LINK_KICK", "Watch on Kick"),
        ("OMNI_LINK_TWITCH", "Watch on Twitch"),
        ("OMNI_LINK_YOUTUBE", "Watch on YouTube"),
    ]

    static var allIdentifiers: Set<String> {
        Set(linkVariants.map(\.identifier)).union([plain])
    }

    /// The category to show: a titled variant when `urlTitle` matches one exactly, the
    /// generic link category for any other URL, and plain without a URL.
    static func category(for payload: OmniPushPayload?, current: String) -> String {
        guard let payload else { return current }
        guard payload.targetURL != nil else { return plain }
        if let title = payload.urlTitle,
           let match = linkVariants.first(where: { $0.title == title })
        {
            return match.identifier
        }
        return link
    }

    static func isLink(_ identifier: String) -> Bool {
        linkVariants.contains { $0.identifier == identifier }
    }
}

/// A communication notification shows the sender's name as its title, which would
/// hide the server's title ("Destiny is LIVE!", "Destiny changed title"). The title
/// moves into the subtitle, or in front of the body when a subtitle already exists.
enum CommunicationText: Equatable {
    struct Lines: Equatable, Sendable {
        let subtitle: String
        let body: String
    }

    static func lines(title: String, subtitle: String, body: String, senderName: String) -> Lines {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, title.caseInsensitiveCompare(senderName) != .orderedSame else {
            return Lines(subtitle: subtitle, body: body)
        }
        if subtitle.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return Lines(subtitle: title, body: body)
        }
        return Lines(subtitle: subtitle, body: body.isEmpty ? title : "\(title)\n\(body)")
    }
}
