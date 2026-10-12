import SwiftUI
import WidgetKit

/// The platform mark: a brand-filled tile with a simple drawn glyph. Brand color is
/// only ever used here (design system 2.5).
struct PlatformGlyph: View {
    let platform: StreamPlatform
    var size: CGFloat = 16

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.28, style: .continuous)
                .fill(Omni.platformColor(platform))
            mark
                .frame(width: size * 0.56, height: size * 0.56)
        }
        .frame(width: size, height: size)
        .accessibilityLabel(platform.displayName)
    }

    @ViewBuilder private var mark: some View {
        switch platform {
        case .youtube:
            PlayTriangle().fill(.white).padding(.leading, size * 0.06)
        case .twitch:
            TwitchMark().fill(.white)
        case .kick:
            KickMark().fill(Color(hex: 0x0B0E0A))
        case .other:
            Circle().fill(.white.opacity(0.9)).padding(size * 0.12)
        }
    }
}

private struct PlayTriangle: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        let inset = rect.insetBy(dx: rect.width * 0.08, dy: rect.height * 0.04)
        path.move(to: CGPoint(x: inset.minX, y: inset.minY))
        path.addLine(to: CGPoint(x: inset.maxX, y: inset.midY))
        path.addLine(to: CGPoint(x: inset.minX, y: inset.maxY))
        path.closeSubpath()
        return path
    }
}

/// A speech-bubble outline with two bars, Twitch's mark simplified.
private struct TwitchMark: Shape {
    func path(in rect: CGRect) -> Path {
        let w = rect.width, h = rect.height
        var outer = Path()
        outer.move(to: CGPoint(x: rect.minX + w * 0.12, y: rect.minY))
        outer.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        outer.addLine(to: CGPoint(x: rect.maxX, y: rect.minY + h * 0.62))
        outer.addLine(to: CGPoint(x: rect.minX + w * 0.66, y: rect.minY + h * 0.86))
        outer.addLine(to: CGPoint(x: rect.minX + w * 0.42, y: rect.minY + h * 0.86))
        outer.addLine(to: CGPoint(x: rect.minX + w * 0.22, y: rect.maxY))
        outer.addLine(to: CGPoint(x: rect.minX + w * 0.22, y: rect.minY + h * 0.86))
        outer.addLine(to: CGPoint(x: rect.minX, y: rect.minY + h * 0.86))
        outer.addLine(to: CGPoint(x: rect.minX, y: rect.minY + h * 0.18))
        outer.closeSubpath()
        var bars = Path()
        bars.addRect(CGRect(x: rect.minX + w * 0.42, y: rect.minY + h * 0.22, width: w * 0.12, height: h * 0.3))
        bars.addRect(CGRect(x: rect.minX + w * 0.68, y: rect.minY + h * 0.22, width: w * 0.12, height: h * 0.3))
        return outer.subtracting(bars)
    }
}

/// A blocky K, Kick's mark simplified.
private struct KickMark: Shape {
    func path(in rect: CGRect) -> Path {
        let w = rect.width, h = rect.height
        let u = w / 5
        var path = Path()
        path.addRect(CGRect(x: rect.minX, y: rect.minY, width: u * 1.6, height: h))
        path.move(to: CGPoint(x: rect.minX + u * 1.6, y: rect.minY + h * 0.42))
        path.addLine(to: CGPoint(x: rect.minX + u * 3.4, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.minX + u * 2.9, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX + u * 3.4, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX + u * 1.6, y: rect.minY + h * 0.58))
        path.closeSubpath()
        return path
    }
}

/// The live dot: crimson, never shown without the word LIVE or a ring nearby.
struct LiveDot: View {
    var size: CGFloat = 7

    var body: some View {
        Circle()
            .fill(Omni.live)
            .frame(width: size, height: size)
            .background(Circle().fill(Omni.live.opacity(0.28)).frame(width: size * 2, height: size * 2))
            .widgetAccentable()
    }
}

/// `● LIVE` tag. Uppercase is reserved for this tag (design system 3.2).
struct LiveTag: View {
    var compact = false

    var body: some View {
        HStack(spacing: 5) {
            LiveDot(size: compact ? 5 : 6)
            Text("LIVE")
                .font(OmniFont.mono(compact ? 11 : 12, .semibold, relativeTo: .caption2))
                .tracking(0.6)
                .foregroundStyle(Omni.live)
                .widgetAccentable()
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Live")
    }
}

/// A streamer avatar: photo or monogram, optional live ring and platform badge.
struct StreamerAvatarView: View {
    let name: String
    let id: String
    let image: AvatarImage?
    var size: CGFloat
    var platform: StreamPlatform?
    var tier: StreamerTier = .primary
    var isLive = true
    /// The crimson ring marks the one hero; rows under an "On air" header skip it so
    /// a list of live streams is not a wall of red.
    var showsRing: Bool? = nil

    var body: some View {
        ZStack(alignment: .bottomTrailing) {
            face
                .frame(width: size, height: size)
                .clipShape(Circle())
                .padding(ring ? ringGap + ringWidth : 0)
                .overlay {
                    if ring {
                        Circle()
                            .strokeBorder(Omni.live, lineWidth: ringWidth)
                            .widgetAccentable()
                    }
                }
            if let platform, size >= 24 {
                PlatformGlyph(platform: platform, size: badgeSize)
                    .padding(max(1.5, size * 0.035))
                    .background(
                        RoundedRectangle(cornerRadius: badgeSize * 0.36, style: .continuous)
                            .fill(Omni.canvas)
                    )
                    .offset(x: ring ? -1 : 2, y: ring ? -1 : 2)
            }
        }
        .accessibilityHidden(true)
    }

    private var ring: Bool { showsRing ?? isLive }
    private var ringWidth: CGFloat { size >= 48 ? 2.5 : 2 }
    private var ringGap: CGFloat { size >= 48 ? 2.5 : 2 }
    private var badgeSize: CGFloat { max(12, (size * 0.34).rounded()) }

    @ViewBuilder private var face: some View {
        if let image {
            Image(decorative: image.image, scale: 1)
                .resizable()
                .widgetAccentedRenderingMode(.desaturated)
                .aspectRatio(contentMode: .fill)
                .saturation(isLive ? 1 : 0.15)
                .opacity(isLive ? 1 : 0.75)
        } else {
            Monogram(name: name, id: id, size: size, tier: tier, dimmed: !isLive)
        }
    }
}

/// Initials on a muted two-stop graphite gradient picked from the id hash, with a
/// faint tier-colored edge (Signal for primary, neutral for background).
struct Monogram: View {
    let name: String
    let id: String
    let size: CGFloat
    var tier: StreamerTier = .primary
    var dimmed = false

    var body: some View {
        let (a, b) = Monogram.stops(for: id)
        ZStack {
            LinearGradient(colors: [a, b], startPoint: .topLeading, endPoint: .bottomTrailing)
            Text(Monogram.initials(name))
                .font(OmniFont.sans(size * 0.38, .semibold))
                .foregroundStyle(Omni.text.opacity(dimmed ? 0.55 : 0.92))
                .minimumScaleFactor(0.5)
        }
        .overlay(
            Circle().strokeBorder(
                tier == .primary ? Omni.signal.opacity(0.35) : Omni.lineStrong,
                lineWidth: max(1, size * 0.03)
            )
        )
    }

    static func initials(_ name: String) -> String {
        let words = name.split(whereSeparator: { $0 == " " || $0 == "_" || $0 == "-" })
        let letters = words.prefix(2).compactMap(\.first)
        let result = letters.isEmpty ? "?" : String(letters)
        return result.uppercased()
    }

    /// Deterministic muted hue pair (FNV-1a over the id, stable across launches).
    static func stops(for id: String) -> (Color, Color) {
        var hash: UInt32 = 2_166_136_261
        for byte in id.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 16_777_619
        }
        let hue = Double(hash % 360) / 360
        return (
            Color(hue: hue, saturation: 0.32, brightness: 0.32),
            Color(hue: (hue + 0.08).truncatingRemainder(dividingBy: 1), saturation: 0.28, brightness: 0.17)
        )
    }
}

/// The warn shape (stale, degraded): a diamond, never color alone.
struct Diamond: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.midX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
        path.closeSubpath()
        return path
    }
}

/// A row divider: a hairline, never a box.
struct Hairline: View {
    var body: some View {
        Rectangle().fill(Omni.line).frame(height: 1 / 3 * 2)
    }
}

extension AvatarImage {
    var color: Color { Color(.sRGB, red: red, green: green, blue: blue) }
}
