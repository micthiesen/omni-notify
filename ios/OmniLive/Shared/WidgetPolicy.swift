import Foundation

/// Pure timeline decisions for the On Air and Deliveries widgets.
enum OnAirPolicy {
    /// A snapshot older than this shows a subtle "as of HH:mm".
    static let staleAfter: TimeInterval = 15 * 60
    /// Past this, cached live data is no longer shown at all.
    static let hardExpiry: TimeInterval = 3 * 60 * 60

    /// Reload sooner while someone is live; widget pushes cover the edges between.
    static func refreshInterval(for snapshot: OnAirSnapshot?) -> TimeInterval {
        guard let snapshot, !snapshot.live.isEmpty else { return 30 * 60 }
        return 15 * 60
    }

    static func isStale(fetchedAt: Date?, now: Date) -> Bool {
        guard let fetchedAt else { return false }
        return now.timeIntervalSince(fetchedAt) > staleAfter
    }

    /// Smart Stack relevance: a primary streamer on air matters most.
    static func relevanceScore(for snapshot: OnAirSnapshot?) -> Float {
        guard let snapshot, !snapshot.live.isEmpty else { return 0 }
        return snapshot.hasPrimaryLive ? 1 : 0.2
    }

    /// How many avatars a family can show, which bounds downloads and decoded memory.
    static func avatarBudget(familyRawName: String) -> Int {
        switch familyRawName {
        case "systemSmall": 4
        case "systemMedium": 3
        case "systemLarge": 6
        case "systemExtraLarge": 8
        case "accessoryCircular": 1
        default: 0
        }
    }
}

enum DeliveriesPolicy {
    static let staleAfter: TimeInterval = 2 * 60 * 60
    static let hardExpiry: TimeInterval = 24 * 60 * 60

    /// The server reads Parcel at most every 30 minutes (3 hours when idle).
    static func refreshInterval(for snapshot: DeliveriesSnapshot?) -> TimeInterval {
        guard let snapshot, snapshot.activeCount > 0 else { return 3 * 60 * 60 }
        let urgent = snapshot.active.contains {
            $0.status == .outForDelivery || $0.status == .awaitingPickup
        }
        return urgent ? 30 * 60 : 60 * 60
    }
}

/// Parcel's verbatim local date strings (`2026-10-14 00:00:00`, `2026-10-12 08:15`,
/// `2026-10-14`), rendered as friendly relative days.
enum DeliveryDates {
    static func parse(_ raw: String?, calendar: Calendar = .current) -> (date: Date, hasTime: Bool)? {
        guard let raw = raw?.trimmingCharacters(in: .whitespaces), !raw.isEmpty else { return nil }
        let parts = raw.split(whereSeparator: { $0 == " " || $0 == "T" })
        let ymd = parts.first?.split(separator: "-").compactMap { Int($0) } ?? []
        guard ymd.count == 3 else { return nil }
        var components = DateComponents(year: ymd[0], month: ymd[1], day: ymd[2])
        var hasTime = false
        if parts.count > 1 {
            let hms = parts[1].split(separator: ":").compactMap { Int($0.prefix(2)) }
            if hms.count >= 2 {
                components.hour = hms[0]
                components.minute = hms[1]
                hasTime = hms[0] != 0 || hms[1] != 0
            }
        }
        guard let date = calendar.date(from: components) else { return nil }
        return (date, hasTime)
    }

    /// "Today", "Tomorrow", "Thu", "Oct 18"; a range collapses to "Thu – Sat".
    /// Past dates on a moving parcel read "Was due Oct 8".
    static func friendlyExpected(
        expected: String?,
        expectedEnd: String?,
        now: Date,
        calendar: Calendar = .current,
        locale: Locale = .current
    ) -> String? {
        guard let start = parse(expected, calendar: calendar) else { return nil }
        let today = calendar.startOfDay(for: now)
        let startDay = calendar.startOfDay(for: start.date)
        let endDay = parse(expectedEnd, calendar: calendar).map { calendar.startOfDay(for: $0.date) }
        let lastDay = max(endDay ?? startDay, startDay)
        if lastDay < today {
            return "Was due \(monthDay(lastDay, calendar, locale))"
        }
        let firstDay = max(startDay, today)
        let first = dayName(firstDay, today: today, calendar: calendar, locale: locale)
        if lastDay > firstDay {
            let last = dayName(lastDay, today: today, calendar: calendar, locale: locale)
            return "\(first) – \(last)"
        }
        return first
    }

    static func isOverdue(expected: String?, expectedEnd: String?, now: Date, calendar: Calendar = .current) -> Bool {
        guard let last = parse(expectedEnd, calendar: calendar) ?? parse(expected, calendar: calendar)
        else { return false }
        return calendar.startOfDay(for: last.date) < calendar.startOfDay(for: now)
    }

    private static func dayName(_ day: Date, today: Date, calendar: Calendar, locale: Locale) -> String {
        let delta = calendar.dateComponents([.day], from: today, to: day).day ?? 0
        switch delta {
        case 0: return "Today"
        case 1: return "Tomorrow"
        case 2 ... 6:
            var style = Date.FormatStyle(locale: locale, calendar: calendar).weekday(.abbreviated)
            style.timeZone = calendar.timeZone
            return day.formatted(style)
        default:
            return monthDay(day, calendar, locale)
        }
    }

    private static func monthDay(_ day: Date, _ calendar: Calendar, _ locale: Locale) -> String {
        var style = Date.FormatStyle(locale: locale, calendar: calendar).month(.abbreviated).day()
        style.timeZone = calendar.timeZone
        return day.formatted(style)
    }
}

/// Compact numbers and short relative times shared by widgets and tests.
enum OmniFormat {
    /// 999 -> "999", 1_234 -> "1.2K", 12_345 -> "12.3K", 123_456 -> "123K", 1_234_567 -> "1.2M".
    static func compact(_ value: Int, locale: Locale = .current) -> String {
        let style = IntegerFormatStyle<Int>.number.notation(.compactName).locale(locale)
        let magnitude = abs(value)
        let leading = magnitude >= 1_000_000 ? magnitude / 1_000_000 : magnitude / 1_000
        return leading >= 100
            ? value.formatted(style.precision(.fractionLength(0)))
            : value.formatted(style.precision(.fractionLength(0 ... 1)))
    }

    /// "3h ago", "12m ago", "2d ago", "now".
    static func ago(_ date: Date, now: Date) -> String {
        let seconds = max(0, now.timeIntervalSince(date))
        switch seconds {
        case ..<60: return "now"
        case ..<3_600: return "\(Int(seconds / 60))m ago"
        case ..<86_400: return "\(Int(seconds / 3_600))h ago"
        default: return "\(Int(seconds / 86_400))d ago"
        }
    }

    /// "4h 12m", "38m" for uptime when a ticking timer is not wanted.
    static func duration(since start: Date, now: Date) -> String {
        let minutes = max(0, Int(now.timeIntervalSince(start) / 60))
        let hours = minutes / 60
        return hours > 0 ? "\(hours)h \(minutes % 60)m" : "\(minutes)m"
    }
}
