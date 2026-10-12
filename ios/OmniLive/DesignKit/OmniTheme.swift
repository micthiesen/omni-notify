import CoreText
import SwiftUI

/// Omni's "Instrument" tokens (docs/design-system.md section 2) for SwiftUI.
enum Omni {
    // Surfaces and lines
    static let canvas = Color(hex: 0x0A0B0D)
    static let surface = Color(hex: 0x111316)
    static let raised = Color(hex: 0x171A1E)
    static let overlay = Color(hex: 0x1D2126)
    static let line = Color(hex: 0x22262C)
    static let lineStrong = Color(hex: 0x30353D)

    // Text
    static let text = Color(hex: 0xECEEF1)
    static let text2 = Color(hex: 0xA9B0BA)
    static let text3 = Color(hex: 0x858D98)
    static let textOff = Color(hex: 0x4B525B)

    // Signal and semantic hues (deviation only)
    static let signal = Color(hex: 0xC6F36A)
    static let signalInk = Color(hex: 0x0C1004)
    static let live = Color(hex: 0xFF4D6A)
    static let ok = Color(hex: 0x4ADE9A)
    static let warn = Color(hex: 0xF4B740)
    static let fault = Color(hex: 0xFF7452)
    static let info = Color(hex: 0x7AB8FF)

    // Platform marks: only ever fill a platform glyph
    static let youtube = Color(hex: 0xFF0033)
    static let twitch = Color(hex: 0xA970FF)
    static let kick = Color(hex: 0x53FC18)

    static let canvasUIColor = UIColor(red: 10 / 255, green: 11 / 255, blue: 13 / 255, alpha: 1)

    static func platformColor(_ platform: StreamPlatform) -> Color {
        switch platform {
        case .kick: kick
        case .twitch: twitch
        case .youtube: youtube
        case .other: text3
        }
    }
}

extension Color {
    init(hex: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: opacity
        )
    }
}

/// Geist and Geist Mono (SIL OFL 1.1), bundled as TTF in each target that draws UI.
/// App and widget extension register them through `UIAppFonts`; hostless test bundles
/// call `registerBundled(in:)`. SF Pro / SF Mono are the fallback if registration fails.
enum OmniFont {
    static let sansFamily = "Geist"
    static let monoFamily = "Geist Mono"

    /// UI text. `relativeTo` keeps Dynamic Type scaling.
    static func sans(_ size: CGFloat, _ weight: Font.Weight = .regular, relativeTo style: Font.TextStyle = .body) -> Font {
        available(sansFamily)
            ? .custom(sansFamily, size: size, relativeTo: style).weight(weight)
            : .system(size: size, weight: weight)
    }

    /// Readouts: viewer counts, durations, times. Always tabular.
    static func mono(_ size: CGFloat, _ weight: Font.Weight = .medium, relativeTo style: Font.TextStyle = .body) -> Font {
        let base: Font = available(monoFamily)
            ? .custom(monoFamily, size: size, relativeTo: style).weight(weight)
            : .system(size: size, weight: weight, design: .monospaced)
        return base.monospacedDigit()
    }

    private static func available(_ family: String) -> Bool {
        UIFont.fontNames(forFamilyName: family).isEmpty == false
    }

    /// Registers the bundled TTFs for this process (tests and previews).
    static func registerBundled(in bundle: Bundle) {
        for name in ["Geist-Variable", "GeistMono-Variable"] {
            guard let url = bundle.url(forResource: name, withExtension: "ttf") else { continue }
            CTFontManagerRegisterFontsForURL(url as CFURL, .process, nil)
        }
    }
}
