import SwiftUI
import WidgetKit

/// On Air in every family. `family` is passed explicitly so snapshot tests can render
/// each size outside WidgetKit.
struct OnAirView: View {
    let entry: OnAirEntry
    let family: WidgetFamily

    var body: some View {
        content
            .containerBackground(for: .widget) { backdrop }
    }

    /// The container background (exposed so snapshot tests can draw it).
    var backdrop: some View {
        WidgetBackdrop(glow: glow, glowCenter: glowCenter)
    }

    @ViewBuilder private var content: some View {
        switch family {
        case .accessoryCircular: OnAirCircular(entry: entry)
        case .accessoryRectangular: OnAirRectangular(entry: entry)
        case .accessoryInline: OnAirInline(entry: entry)
        case .systemMedium: OnAirMedium(entry: entry)
        case .systemLarge: OnAirLarge(entry: entry)
        case .systemExtraLarge: OnAirExtraLarge(entry: entry)
        default: OnAirSmall(entry: entry)
        }
    }

    /// The warm light behind the hero: its avatar's own color pulled toward live
    /// crimson, so each streamer tints the widget a little differently.
    private var glow: Color? {
        guard let snapshot = entry.snapshot,
              let hero = snapshot.live.first
        else { return nil }
        switch family {
        case .systemSmall, .systemExtraLarge:
            return WidgetGlow.color(for: entry.avatars[hero.id])
        case .systemMedium where snapshot.live.count == 1:
            return WidgetGlow.color(for: entry.avatars[hero.id])
        case .systemMedium, .systemLarge:
            // A list has no single hero: the plain live wash, like the web Stage.
            return Omni.live.opacity(0.55)
        default:
            return nil
        }
    }

    private var glowCenter: UnitPoint {
        switch family {
        case .systemSmall: UnitPoint(x: 0.2, y: 0.22)
        case .systemExtraLarge: UnitPoint(x: 0.14, y: 0.3)
        case .systemMedium where (entry.snapshot?.live.count ?? 0) == 1: UnitPoint(x: 0.14, y: 0.35)
        default: UnitPoint(x: 0.05, y: 0.0)
        }
    }
}

enum WidgetGlow {
    static func color(for avatar: AvatarImage?) -> Color {
        guard let avatar else { return Omni.live }
        let peak = max(avatar.red, avatar.green, avatar.blue, 0.001)
        let (r, g, b) = (avatar.red / peak, avatar.green / peak, avatar.blue / peak)
        // 60% the avatar's normalized hue, 40% live crimson (#ff4d6a).
        return Color(.sRGB, red: r * 0.6 + 1.0 * 0.4, green: g * 0.6 + 0.302 * 0.4, blue: b * 0.6 + 0.416 * 0.4)
    }
}

/// Graphite with a static radial light. No blur: gradients only (design system 1.8).
struct WidgetBackdrop: View {
    var glow: Color?
    var glowCenter: UnitPoint = .topLeading

    @Environment(\.widgetRenderingMode) private var renderingMode

    var body: some View {
        let glow = renderingMode == .fullColor ? glow : nil
        return ZStack {
            Omni.canvas
            LinearGradient(
                colors: [Omni.surface, Omni.canvas],
                startPoint: .top,
                endPoint: .bottom
            )
            if let glow {
                GeometryReader { proxy in
                    let radius = max(proxy.size.width, proxy.size.height) * 0.75
                    RadialGradient(
                        colors: [glow.opacity(0.34), glow.opacity(0.10), .clear],
                        center: glowCenter,
                        startRadius: 0,
                        endRadius: radius
                    )
                }
            }
        }
    }
}

// MARK: - Shared pieces

/// A ticking uptime readout (`Text(.timer)` updates without timeline reloads).
struct UptimeText: View {
    let start: Date?
    var size: CGFloat = 12
    var color: Color = Omni.text3
    var alignment: TextAlignment = .leading

    var body: some View {
        if let start {
            // Timer text claims its widest width in widgets; pin it to a frame.
            Text(start, style: .timer)
                .font(OmniFont.mono(size, .regular, relativeTo: .caption))
                .foregroundStyle(color)
                .multilineTextAlignment(alignment)
                .lineLimit(1)
                .frame(width: size * 0.62 * 8.6, alignment: alignment == .trailing ? .trailing : .leading)
        }
    }
}

struct ViewerCount: View {
    let value: Int?
    var size: CGFloat
    var weight: Font.Weight = .medium
    var color: Color = Omni.text

    var body: some View {
        Text(value.map { OmniFormat.compact($0) } ?? "–")
            .font(OmniFont.mono(size, weight, relativeTo: .title3))
            .tracking(-size * 0.03)
            .foregroundStyle(value == nil ? Omni.text3 : color)
            .lineLimit(1)
            .minimumScaleFactor(0.7)
    }
}

/// "as of 14:05" when the snapshot is older than 15 minutes.
struct StaleStamp: View {
    let fetchedAt: Date?
    let isStale: Bool
    /// Small widgets drop the "as of" words and keep the diamond plus time.
    var compact = false

    var body: some View {
        if isStale, let fetchedAt {
            HStack(spacing: 4) {
                Diamond().fill(Omni.warn).frame(width: 6, height: 6)
                Text(compact ? time(fetchedAt) : "as of \(time(fetchedAt))")
                    .font(OmniFont.mono(11, .regular, relativeTo: .caption2))
                    .foregroundStyle(Omni.text3)
                    .lineLimit(1)
            }
            .accessibilityLabel("Last updated \(time(fetchedAt))")
        }
    }

    private func time(_ date: Date) -> String {
        compact
            ? date.formatted(.dateTime.hour(.defaultDigits(amPM: .omitted)).minute())
            : date.formatted(date: .omitted, time: .shortened)
    }
}

/// Overlapping mini avatars for the streamers behind the hero, plus "+N".
struct AvatarStack: View {
    let streamers: [OnAirStreamer]
    let extra: Int
    let avatars: [String: AvatarImage]
    var size: CGFloat = 22

    var body: some View {
        HStack(spacing: -size * 0.32) {
            ForEach(streamers) { streamer in
                StreamerAvatarView(
                    name: streamer.name, id: streamer.id, image: avatars[streamer.id],
                    size: size, platform: nil, tier: streamer.tier, isLive: false
                )
                .padding(1.5)
                .background(Circle().fill(Omni.canvas))
            }
            if extra > 0 {
                Text("+\(extra)")
                    .font(OmniFont.mono(11, .medium, relativeTo: .caption2))
                    .foregroundStyle(Omni.text2)
                    .padding(.leading, size * 0.32 + 4)
            }
        }
    }
}

/// One live stream as a row: avatar, name over title, viewers over uptime.
struct StreamRow: View {
    let streamer: OnAirStreamer
    let avatar: AvatarImage?
    var avatarSize: CGFloat = 32
    var showsCategory = false
    var nameSize: CGFloat = 15

    var body: some View {
        HStack(spacing: 12) {
            StreamerAvatarView(
                name: streamer.name, id: streamer.id, image: avatar, size: avatarSize,
                platform: streamer.platform, tier: streamer.tier, showsRing: false
            )
            VStack(alignment: .leading, spacing: 2) {
                Text(streamer.name)
                    .font(OmniFont.sans(nameSize, .semibold, relativeTo: .subheadline))
                    .foregroundStyle(Omni.text)
                    .lineLimit(1)
                if let subtitle {
                    Text(subtitle)
                        .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                        .foregroundStyle(Omni.text2)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            VStack(alignment: .trailing, spacing: 2) {
                ViewerCount(value: streamer.displayViewers, size: 16)
                UptimeText(start: streamer.startedDate, size: 11, alignment: .trailing)
            }
            .fixedSize()
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(accessibilityText)
    }

    private var subtitle: String? {
        let title = streamer.title?.trimmingCharacters(in: .whitespacesAndNewlines)
        let category = streamer.category?.trimmingCharacters(in: .whitespacesAndNewlines)
        let parts = [title, showsCategory ? category : nil].compactMap { $0?.isEmpty == false ? $0 : nil }
        if parts.isEmpty { return category?.isEmpty == false ? category : streamer.platform.displayName }
        return parts.joined(separator: " · ")
    }

    private var accessibilityText: String {
        var parts = ["\(streamer.name), live on \(streamer.platform.displayName)"]
        if let viewers = streamer.displayViewers { parts.append("\(viewers) viewers") }
        if let title = streamer.title, !title.isEmpty { parts.append(title) }
        return parts.joined(separator: ", ")
    }
}

/// A recently ended stream: desaturated avatar, name, "3h ago".
struct RecentRow: View {
    let recent: OnAirRecent
    let avatar: AvatarImage?
    let now: Date
    var avatarSize: CGFloat = 24
    var nameSize: CGFloat = 13
    /// "3h" instead of "3h ago" where a "Last live" label already says it.
    var compactAgo = false

    var body: some View {
        HStack(spacing: 10) {
            StreamerAvatarView(
                name: recent.name, id: recent.id, image: avatar, size: avatarSize,
                platform: avatarSize >= 24 ? recent.platform : nil, isLive: false
            )
            Text(recent.name)
                .font(OmniFont.sans(nameSize, .medium, relativeTo: .footnote))
                .foregroundStyle(Omni.text2)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            if let ended = recent.endedDate {
                Text(compactAgo ? OmniFormat.ago(ended, now: now).replacingOccurrences(of: " ago", with: "")
                    : OmniFormat.ago(ended, now: now))
                    .font(OmniFont.mono(11, .regular, relativeTo: .caption2))
                    .foregroundStyle(Omni.text3)
                    .lineLimit(1)
                    .fixedSize()
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// Wraps a row so it opens its stream directly (see `OpenStreamIntent`).
struct OpenButton<Label: View>: View {
    let url: URL?
    @ViewBuilder let label: () -> Label

    var body: some View {
        if let url {
            Button(intent: OpenStreamIntent(url: url)) { label().contentShape(Rectangle()) }
                .buttonStyle(.plain)
        } else {
            label()
        }
    }
}

/// Header: "● On air · 5 live" left, total viewers right.
struct OnAirHeader: View {
    let snapshot: OnAirSnapshot
    var size: CGFloat = 15

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            LiveDot(size: 7).alignmentGuide(.firstTextBaseline) { $0[.bottom] - 1 }
            Text("On air")
                .font(OmniFont.sans(size, .semibold, relativeTo: .headline))
                .foregroundStyle(Omni.text)
            Text("\(snapshot.liveCount) live")
                .font(OmniFont.mono(size - 2, .regular, relativeTo: .subheadline))
                .foregroundStyle(Omni.text3)
            Spacer(minLength: 8)
            if snapshot.totalViewers > 0 {
                Text(OmniFormat.compact(snapshot.totalViewers))
                    .font(OmniFont.mono(size, .medium, relativeTo: .headline))
                    .foregroundStyle(Omni.text)
                Text("watching")
                    .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                    .foregroundStyle(Omni.text3)
            }
        }
        .lineLimit(1)
    }
}

/// Problem or empty states shared by every system family.
struct OnAirMessage: View {
    let title: String
    let detail: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(OmniFont.sans(17, .semibold, relativeTo: .headline))
                .foregroundStyle(Omni.text)
            if let detail {
                Text(detail)
                    .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                    .foregroundStyle(Omni.text3)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
    }

    static func forProblem(_ problem: WidgetProblem?) -> OnAirMessage {
        switch problem {
        case .notConfigured?:
            OnAirMessage(title: "Connect Omni", detail: "Open Omni to add the server and token.")
        default:
            OnAirMessage(title: "Omni is out of reach", detail: "Updates when omni.boris is reachable.")
        }
    }
}

// MARK: - Small

struct OnAirSmall: View {
    let entry: OnAirEntry

    var body: some View {
        Group {
            if let snapshot = entry.snapshot {
                if let hero = snapshot.live.first {
                    live(hero, snapshot)
                        .widgetURL(OmniWidgetLinks.open(hero))
                } else {
                    nobody(snapshot)
                        .widgetURL(OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
                }
            } else {
                OnAirMessage.forProblem(entry.problem)
            }
        }
    }

    private func live(_ hero: OnAirStreamer, _ snapshot: OnAirSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .top, spacing: 0) {
                StreamerAvatarView(
                    name: hero.name, id: hero.id, image: entry.avatars[hero.id], size: 50,
                    platform: hero.platform, tier: hero.tier
                )
                Spacer(minLength: 4)
                let others = Array(snapshot.live.dropFirst().prefix(2))
                if !others.isEmpty {
                    AvatarStack(
                        streamers: others,
                        extra: max(0, snapshot.liveCount - 1 - others.count),
                        avatars: entry.avatars,
                        size: 20
                    )
                    .padding(.top, 2)
                }
            }
            Spacer(minLength: 6)
            Text(hero.name)
                .font(OmniFont.sans(16, .semibold, relativeTo: .headline))
                .foregroundStyle(Omni.text)
                .lineLimit(1)
                .minimumScaleFactor(0.75)
            ViewerCount(value: hero.displayViewers, size: 28)
                .padding(.top, 1)
            HStack(spacing: 6) {
                LiveTag(compact: true)
                if entry.isStale {
                    StaleStamp(fetchedAt: entry.fetchedAt, isStale: true, compact: true)
                } else {
                    UptimeText(start: hero.startedDate, size: 11)
                }
            }
            .padding(.top, 3)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func nobody(_ snapshot: OnAirSnapshot) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Nobody's live")
                .font(OmniFont.sans(16, .semibold, relativeTo: .headline))
                .foregroundStyle(Omni.text)
            Text(snapshot.recentOffline.isEmpty ? "Omni is watching" : "Last live")
                .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                .foregroundStyle(Omni.text3)
                .padding(.top, 2)
            Spacer(minLength: 8)
            VStack(alignment: .leading, spacing: 8) {
                ForEach(snapshot.recentOffline.prefix(3)) { recent in
                    RecentRow(recent: recent, avatar: entry.avatars[recent.id], now: entry.date,
                              avatarSize: 20, nameSize: 13, compactAgo: true)
                }
            }
            StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale).padding(.top, 6)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

// MARK: - Medium

struct OnAirMedium: View {
    let entry: OnAirEntry

    var body: some View {
        Group {
            if let snapshot = entry.snapshot {
                if snapshot.live.count == 1, let hero = snapshot.live.first {
                    heroCard(hero, snapshot)
                } else if snapshot.live.isEmpty {
                    OnAirEmptyList(entry: entry, snapshot: snapshot, rows: 3)
                } else {
                    rows(snapshot)
                }
            } else {
                OnAirMessage.forProblem(entry.problem)
            }
        }
        .widgetURL(OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
    }

    private func rows(_ snapshot: OnAirSnapshot) -> some View {
        let shown = Array(snapshot.live.prefix(3))
        return VStack(spacing: 0) {
            ForEach(Array(shown.enumerated()), id: \.element.id) { index, streamer in
                if index > 0 { Hairline().padding(.leading, 44) }
                OpenButton(url: streamer.watchUrl) {
                    StreamRow(streamer: streamer, avatar: entry.avatars[streamer.id], avatarSize: 32)
                        .padding(.vertical, shown.count == 3 ? 3 : 6)
                }
            }
            Spacer(minLength: 0)
            footer(snapshot, shown: shown.count)
        }
    }

    @ViewBuilder private func footer(_ snapshot: OnAirSnapshot, shown: Int) -> some View {
        let more = snapshot.liveCount - shown
        if entry.isStale || more > 0 {
            HStack {
                if more > 0 {
                    Text("+\(more) more live")
                        .font(OmniFont.sans(11, .medium, relativeTo: .caption2))
                        .foregroundStyle(Omni.text3)
                }
                Spacer()
                StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale)
            }
            .padding(.top, 2)
        }
    }

    private func heroCard(_ hero: OnAirStreamer, _ snapshot: OnAirSnapshot) -> some View {
        OpenButton(url: hero.watchUrl) {
            HStack(alignment: .center, spacing: 16) {
                StreamerAvatarView(
                    name: hero.name, id: hero.id, image: entry.avatars[hero.id], size: 76,
                    platform: hero.platform, tier: hero.tier
                )
                VStack(alignment: .leading, spacing: 0) {
                    HStack(spacing: 8) {
                        LiveTag()
                        UptimeText(start: hero.startedDate, size: 12)
                    }
                    Text(hero.name)
                        .font(OmniFont.sans(20, .semibold, relativeTo: .title3))
                        .tracking(-0.3)
                        .foregroundStyle(Omni.text)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .padding(.top, 4)
                    if let title = hero.title, !title.isEmpty {
                        Text(title)
                            .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                            .foregroundStyle(Omni.text2)
                            .lineLimit(2)
                            .padding(.top, 1)
                    }
                    Spacer(minLength: 4)
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        ViewerCount(value: hero.displayViewers, size: 24)
                        Text("viewers")
                            .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                            .foregroundStyle(Omni.text3)
                        Spacer(minLength: 4)
                        StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
        }
    }
}

/// Nobody live: a calm sentence plus who was on last.
struct OnAirEmptyList: View {
    let entry: OnAirEntry
    let snapshot: OnAirSnapshot
    let rows: Int
    var showsTitle = true

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if showsTitle {
                HStack(alignment: .firstTextBaseline) {
                    Text("Nobody's live")
                        .font(OmniFont.sans(17, .semibold, relativeTo: .headline))
                        .foregroundStyle(Omni.text)
                    Spacer()
                    StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale)
                }
            }
            if snapshot.recentOffline.isEmpty {
                Spacer()
                Text("Omni will light this up when a stream starts.")
                    .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                    .foregroundStyle(Omni.text3)
            } else {
                Text("Last live")
                    .font(OmniFont.sans(12, .medium, relativeTo: .caption))
                    .foregroundStyle(Omni.text3)
                    .padding(.top, 2)
                if showsTitle { Spacer(minLength: 6) }
                VStack(spacing: 0) {
                    ForEach(Array(snapshot.recentOffline.prefix(rows).enumerated()), id: \.element.id) { index, recent in
                        if index > 0 { Hairline().padding(.leading, 38) }
                        OpenButton(url: recent.watchUrl) {
                            RecentRow(recent: recent, avatar: entry.avatars[recent.id], now: entry.date,
                                      avatarSize: 28, nameSize: 14)
                                .padding(.vertical, 5)
                        }
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

// MARK: - Large

struct OnAirLarge: View {
    let entry: OnAirEntry

    var body: some View {
        Group {
            if let snapshot = entry.snapshot {
                if snapshot.live.isEmpty {
                    OnAirEmptyList(entry: entry, snapshot: snapshot, rows: 4)
                } else {
                    list(snapshot)
                }
            } else {
                OnAirMessage.forProblem(entry.problem)
            }
        }
        .widgetURL(OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
    }

    private func list(_ snapshot: OnAirSnapshot) -> some View {
        let shown = Array(snapshot.live.prefix(6))
        let recentRoom = max(0, 5 - shown.count)
        return VStack(alignment: .leading, spacing: 0) {
            OnAirHeader(snapshot: snapshot)
                .padding(.bottom, 8)
            Hairline()
            ForEach(Array(shown.enumerated()), id: \.element.id) { index, streamer in
                if index > 0 { Hairline().padding(.leading, 44) }
                OpenButton(url: streamer.watchUrl) {
                    StreamRow(streamer: streamer, avatar: entry.avatars[streamer.id], avatarSize: 32,
                              showsCategory: true)
                        .padding(.vertical, 5)
                }
            }
            if recentRoom > 0, !snapshot.recentOffline.isEmpty {
                Text("Last live")
                    .font(OmniFont.sans(12, .medium, relativeTo: .caption))
                    .foregroundStyle(Omni.text3)
                    .padding(.top, 14)
                    .padding(.bottom, 4)
                ForEach(snapshot.recentOffline.prefix(recentRoom)) { recent in
                    OpenButton(url: recent.watchUrl) {
                        RecentRow(recent: recent, avatar: entry.avatars[recent.id], now: entry.date)
                            .padding(.vertical, 4)
                    }
                }
            }
            Spacer(minLength: 0)
            footer(snapshot, shown: shown.count)
        }
    }

    @ViewBuilder private func footer(_ snapshot: OnAirSnapshot, shown: Int) -> some View {
        let more = snapshot.liveCount - shown
        if entry.isStale || more > 0 {
            HStack {
                if more > 0 {
                    Text("+\(more) more live")
                        .font(OmniFont.sans(12, .medium, relativeTo: .caption))
                        .foregroundStyle(Omni.text3)
                }
                Spacer()
                StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale)
            }
        }
    }
}

// MARK: - Extra large (iPad)

struct OnAirExtraLarge: View {
    let entry: OnAirEntry

    var body: some View {
        Group {
            if let snapshot = entry.snapshot {
                if let hero = snapshot.live.first {
                    HStack(alignment: .top, spacing: 28) {
                        heroPanel(hero, snapshot)
                            .frame(maxWidth: .infinity)
                        side(snapshot)
                            .frame(maxWidth: .infinity)
                    }
                } else {
                    HStack(alignment: .top, spacing: 28) {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Nobody's live")
                                .font(OmniFont.sans(28, .semibold, relativeTo: .title))
                                .tracking(-0.8)
                                .foregroundStyle(Omni.text)
                            Text(quietLine(snapshot))
                                .font(OmniFont.sans(15, .regular, relativeTo: .body))
                                .foregroundStyle(Omni.text3)
                            Spacer(minLength: 0)
                            StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale)
                        }
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        OnAirEmptyList(entry: entry, snapshot: snapshot, rows: 5, showsTitle: false)
                            .frame(maxWidth: .infinity)
                    }
                }
            } else {
                OnAirMessage.forProblem(entry.problem)
            }
        }
        .widgetURL(OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
    }

    private func quietLine(_ snapshot: OnAirSnapshot) -> String {
        guard let last = snapshot.recentOffline.first, let ended = last.endedDate else {
            return "Omni will light this up when a stream starts."
        }
        return "\(last.name) was on \(OmniFormat.ago(ended, now: entry.date))."
    }

    private func heroPanel(_ hero: OnAirStreamer, _ snapshot: OnAirSnapshot) -> some View {
        OpenButton(url: hero.watchUrl) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .center, spacing: 16) {
                    StreamerAvatarView(
                        name: hero.name, id: hero.id, image: entry.avatars[hero.id], size: 84,
                        platform: hero.platform, tier: hero.tier
                    )
                    VStack(alignment: .leading, spacing: 6) {
                        HStack(spacing: 8) {
                            LiveTag()
                            UptimeText(start: hero.startedDate, size: 13)
                        }
                        Text(hero.name)
                            .font(OmniFont.sans(28, .semibold, relativeTo: .title))
                            .tracking(-0.8)
                            .foregroundStyle(Omni.text)
                            .lineLimit(1)
                            .minimumScaleFactor(0.7)
                    }
                }
                if let title = hero.title, !title.isEmpty {
                    Text(title)
                        .font(OmniFont.sans(16, .medium, relativeTo: .body))
                        .foregroundStyle(Omni.text)
                        .lineLimit(2)
                        .padding(.top, 16)
                }
                if let category = hero.category, !category.isEmpty {
                    Text(category)
                        .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                        .foregroundStyle(Omni.text3)
                        .padding(.top, 3)
                }
                if let headline = hero.headline, !headline.isEmpty {
                    HStack(alignment: .top, spacing: 8) {
                        Capsule().fill(Omni.signal).frame(width: 2.5)
                            .widgetAccentable()
                        Text(headline)
                            .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                            .foregroundStyle(Omni.text2)
                            .lineLimit(2)
                    }
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.top, 12)
                }
                Spacer(minLength: 8)
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    ViewerCount(value: hero.displayViewers, size: 44)
                    Text("viewers")
                        .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                        .foregroundStyle(Omni.text3)
                    Spacer(minLength: 8)
                    if let peak = hero.peakViewers, peak > (hero.viewers ?? 0) {
                        Text("peak \(OmniFormat.compact(peak))")
                            .font(OmniFont.mono(12, .regular, relativeTo: .caption))
                            .foregroundStyle(Omni.text3)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
    }

    private func side(_ snapshot: OnAirSnapshot) -> some View {
        let others = Array(snapshot.live.dropFirst().prefix(5))
        let recentRoom = max(0, 4 - others.count)
        return VStack(alignment: .leading, spacing: 0) {
            OnAirHeader(snapshot: snapshot, size: 15)
                .padding(.bottom, 8)
            Hairline()
            ForEach(Array(others.enumerated()), id: \.element.id) { index, streamer in
                if index > 0 { Hairline().padding(.leading, 44) }
                OpenButton(url: streamer.watchUrl) {
                    StreamRow(streamer: streamer, avatar: entry.avatars[streamer.id], avatarSize: 32,
                              showsCategory: true)
                        .padding(.vertical, 7)
                }
            }
            if recentRoom > 0, !snapshot.recentOffline.isEmpty {
                Text("Last live")
                    .font(OmniFont.sans(12, .medium, relativeTo: .caption))
                    .foregroundStyle(Omni.text3)
                    .padding(.top, others.isEmpty ? 10 : 14)
                    .padding(.bottom, 4)
                ForEach(snapshot.recentOffline.prefix(recentRoom)) { recent in
                    OpenButton(url: recent.watchUrl) {
                        RecentRow(recent: recent, avatar: entry.avatars[recent.id], now: entry.date)
                            .padding(.vertical, 4)
                    }
                }
            }
            Spacer(minLength: 0)
            HStack {
                let more = snapshot.liveCount - 1 - others.count
                if more > 0 {
                    Text("+\(more) more live")
                        .font(OmniFont.sans(12, .medium, relativeTo: .caption))
                        .foregroundStyle(Omni.text3)
                }
                Spacer()
                StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale)
            }
        }
    }
}

// MARK: - Lock screen

struct OnAirCircular: View {
    let entry: OnAirEntry

    var body: some View {
        let count = entry.snapshot?.liveCount ?? 0
        ZStack {
            AccessoryWidgetBackground()
            Circle()
                .strokeBorder(style: StrokeStyle(lineWidth: 2.5, dash: count > 0 ? [] : [2, 3]))
                .opacity(count > 0 ? 1 : 0.4)
                .padding(2)
                .widgetAccentable()
            VStack(spacing: -2) {
                Text(entry.snapshot == nil ? "–" : "\(count)")
                    .font(OmniFont.mono(22, .semibold, relativeTo: .title2))
                    .minimumScaleFactor(0.6)
                Text("live")
                    .font(OmniFont.sans(11, .medium, relativeTo: .caption2))
                    .opacity(0.8)
            }
        }
        .widgetURL(entry.snapshot?.live.first.map(OmniWidgetLinks.open)
            ?? OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
        .accessibilityLabel("\(count) live")
    }
}

struct OnAirRectangular: View {
    let entry: OnAirEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            if let snapshot = entry.snapshot {
                HStack(spacing: 5) {
                    Circle().frame(width: 6, height: 6)
                        .opacity(snapshot.live.isEmpty ? 0.35 : 1)
                        .widgetAccentable()
                    Text(snapshot.live.isEmpty ? "Nobody's live" : "\(snapshot.liveCount) live")
                        .font(OmniFont.sans(14, .semibold, relativeTo: .headline))
                    if snapshot.totalViewers > 0 {
                        Text("· \(OmniFormat.compact(snapshot.totalViewers))")
                            .font(OmniFont.mono(13, .regular, relativeTo: .subheadline))
                            .opacity(0.7)
                    }
                }
                if snapshot.live.isEmpty {
                    ForEach(snapshot.recentOffline.prefix(2)) { recent in
                        line(recent.name, recent.endedDate.map { OmniFormat.ago($0, now: entry.date) })
                            .opacity(0.7)
                    }
                } else {
                    ForEach(snapshot.live.prefix(2)) { streamer in
                        line(streamer.name, streamer.displayViewers.map { OmniFormat.compact($0) })
                    }
                }
            } else {
                Text("Omni").font(OmniFont.sans(14, .semibold, relativeTo: .headline))
                Text(entry.problem == .notConfigured ? "Open to connect" : "Out of reach")
                    .font(OmniFont.sans(13, .regular, relativeTo: .subheadline)).opacity(0.7)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .widgetURL(entry.snapshot?.live.first.map(OmniWidgetLinks.open)
            ?? OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
    }

    private func line(_ name: String, _ value: String?) -> some View {
        HStack(spacing: 6) {
            Text(name)
                .font(OmniFont.sans(14, .regular, relativeTo: .subheadline))
                .lineLimit(1)
            Spacer(minLength: 4)
            if let value {
                Text(value)
                    .font(OmniFont.mono(13, .medium, relativeTo: .subheadline))
                    .lineLimit(1)
                    .fixedSize()
            }
        }
    }
}

struct OnAirInline: View {
    let entry: OnAirEntry

    var body: some View {
        ViewThatFits {
            Text(full)
            Text(short)
        }
        .widgetURL(entry.snapshot?.live.first.map(OmniWidgetLinks.open)
            ?? OmniWidgetLinks.open(page: OmniWidgetLinks.livePage))
    }

    /// "3 live · Destiny 12.4K"
    private var full: String {
        guard let snapshot = entry.snapshot else { return "Omni · out of reach" }
        guard let top = snapshot.live.first else { return "Nobody's live" }
        let viewers = top.displayViewers.map { " " + OmniFormat.compact($0) } ?? ""
        return "\(snapshot.liveCount) live · \(top.name)\(viewers)"
    }

    private var short: String {
        guard let snapshot = entry.snapshot else { return "Omni" }
        guard let top = snapshot.live.first else { return "Nobody's live" }
        return snapshot.liveCount > 1 ? "\(snapshot.liveCount) live · \(top.name)" : "\(top.name) live"
    }
}
