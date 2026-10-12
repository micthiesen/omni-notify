import SwiftUI
import WidgetKit

struct DeliveriesView: View {
    let entry: DeliveriesEntry
    let family: WidgetFamily

    var body: some View {
        content
            .widgetURL(OmniWidgetLinks.open(page: OmniWidgetLinks.deliveriesPage))
            .containerBackground(for: .widget) { WidgetBackdrop(glow: nil) }
    }

    @ViewBuilder private var content: some View {
        if let snapshot = entry.snapshot, snapshot.configured {
            switch family {
            case .accessoryRectangular: DeliveriesRectangular(entry: entry, snapshot: snapshot)
            case .systemMedium: DeliveriesList(entry: entry, snapshot: snapshot, rows: 3, detail: false)
            case .systemLarge: DeliveriesList(entry: entry, snapshot: snapshot, rows: 5, detail: true)
            default: DeliveriesSmall(entry: entry, snapshot: snapshot)
            }
        } else if family == .accessoryRectangular {
            VStack(alignment: .leading, spacing: 1) {
                Text("Deliveries").font(OmniFont.sans(14, .semibold, relativeTo: .headline))
                Text(problemDetail).font(OmniFont.sans(13, .regular, relativeTo: .subheadline)).opacity(0.7)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            OnAirMessage(title: problemTitle, detail: problemDetail)
        }
    }

    private var problemTitle: String {
        if entry.snapshot?.configured == false { return "Parcel isn't set up" }
        return entry.problem == .notConfigured ? "Connect Omni" : "Omni is out of reach"
    }

    private var problemDetail: String {
        if entry.snapshot?.configured == false { return "Add a Parcel API key to Omni." }
        return entry.problem == .notConfigured
            ? "Open Omni to add the server and token."
            : "Updates when omni.boris is reachable."
    }
}

/// Sort: what needs you (exceptions, pickup), then out for delivery, then soonest.
enum DeliveryOrdering {
    static func ranked(_ deliveries: [Delivery], now: Date) -> [Delivery] {
        deliveries.enumerated().sorted { a, b in
            let ra = rank(a.element), rb = rank(b.element)
            if ra != rb { return ra < rb }
            let da = DeliveryDates.parse(a.element.expected)?.date ?? .distantFuture
            let db = DeliveryDates.parse(b.element.expected)?.date ?? .distantFuture
            if da != db { return da < db }
            return a.offset < b.offset
        }.map(\.element)
    }

    private static func rank(_ delivery: Delivery) -> Int {
        switch delivery.status {
        case .outForDelivery: 0
        case .awaitingPickup, .failedAttempt, .exception: 1
        case .inTransit: 2
        case .frozen, .infoReceived, .notFound, .unknown: 3
        case .completed: 4
        }
    }
}

/// A small journey meter (four pips) for parcels on the happy path; a fault square
/// or warn diamond for the ones that need attention. The status word always sits
/// beside it, so color is never alone.
struct DeliveryGlyph: View {
    let status: DeliveryStatus
    var pipWidth: CGFloat = 7

    var body: some View {
        switch status.tone {
        case .fault:
            RoundedRectangle(cornerRadius: 1.5).fill(Omni.fault).frame(width: 8, height: 8)
                .widgetAccentable()
        case .warn where status.progressStep == nil:
            Diamond().fill(Omni.warn).frame(width: 9, height: 9).widgetAccentable()
        default:
            HStack(spacing: 2) {
                ForEach(1 ... 4, id: \.self) { step in
                    Capsule()
                        .fill(step <= (status.progressStep ?? 0) ? fill : Omni.lineStrong)
                        .frame(width: pipWidth, height: 3.5)
                }
            }
            .widgetAccentable()
        }
    }

    private var fill: Color {
        switch status.tone {
        case .info: Omni.info
        case .warn: Omni.warn
        case .done: Omni.text3
        default: Omni.text
        }
    }
}

struct DeliveryStatusText: View {
    let delivery: Delivery
    var size: CGFloat = 12

    var body: some View {
        Text(delivery.statusLabel)
            .font(OmniFont.sans(size, .medium, relativeTo: .caption))
            .foregroundStyle(color)
            .lineLimit(1)
    }

    private var color: Color {
        switch delivery.status.tone {
        case .info: Omni.info
        case .warn: Omni.warn
        case .fault: Omni.fault
        case .neutral, .done: Omni.text2
        }
    }
}

struct ExpectedText: View {
    let delivery: Delivery
    let now: Date
    var size: CGFloat = 13

    var body: some View {
        if let label {
            Text(label)
                .font(OmniFont.mono(size, .medium, relativeTo: .footnote))
                .foregroundStyle(overdue ? Omni.warn : Omni.text)
                .lineLimit(1)
        }
    }

    private var label: String? {
        DeliveryDates.friendlyExpected(expected: delivery.expected, expectedEnd: delivery.expectedEnd, now: now)
    }

    private var overdue: Bool {
        delivery.active
            && DeliveryDates.isOverdue(expected: delivery.expected, expectedEnd: delivery.expectedEnd, now: now)
    }
}

struct DeliveriesSmall: View {
    let entry: DeliveriesEntry
    let snapshot: DeliveriesSnapshot

    var body: some View {
        let ranked = DeliveryOrdering.ranked(snapshot.active, now: entry.date)
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Text(ranked.isEmpty ? "Deliveries" : "On the way")
                    .font(OmniFont.sans(13, .medium, relativeTo: .footnote))
                    .foregroundStyle(Omni.text3)
                Spacer()
                if !ranked.isEmpty {
                    Text("\(snapshot.activeCount)")
                        .font(OmniFont.mono(13, .medium, relativeTo: .footnote))
                        .foregroundStyle(Omni.text2)
                }
            }
            if let lead = ranked.first {
                Spacer(minLength: 6)
                DeliveryGlyph(status: lead.status)
                Text(lead.displayTitle)
                    .font(OmniFont.sans(17, .semibold, relativeTo: .headline))
                    .foregroundStyle(Omni.text)
                    .lineLimit(2)
                    .minimumScaleFactor(0.85)
                    .padding(.top, 8)
                DeliveryStatusText(delivery: lead, size: 13).padding(.top, 2)
                Spacer(minLength: 4)
                HStack(alignment: .lastTextBaseline) {
                    ExpectedText(delivery: lead, now: entry.date, size: 15)
                    Spacer(minLength: 2)
                    if ranked.count > 1 {
                        Text("+\(ranked.count - 1)")
                            .font(OmniFont.mono(12, .regular, relativeTo: .caption))
                            .foregroundStyle(Omni.text3)
                    }
                }
            } else {
                Spacer(minLength: 6)
                Text("Nothing on the way")
                    .font(OmniFont.sans(17, .semibold, relativeTo: .headline))
                    .foregroundStyle(Omni.text)
                if let last = snapshot.completed.first {
                    Text("\(last.displayTitle) delivered")
                        .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                        .foregroundStyle(Omni.text3)
                        .lineLimit(2)
                        .padding(.top, 3)
                }
            }
            StaleStamp(fetchedAt: entry.fetchedAt, isStale: entry.isStale).padding(.top, 4)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

struct DeliveriesList: View {
    let entry: DeliveriesEntry
    let snapshot: DeliveriesSnapshot
    let rows: Int
    let detail: Bool

    var body: some View {
        let ranked = DeliveryOrdering.ranked(snapshot.active, now: entry.date)
        let shown = Array(ranked.prefix(rows))
        VStack(alignment: .leading, spacing: 0) {
            header(count: ranked.count)
                .padding(.bottom, detail ? 8 : 4)
            if shown.isEmpty {
                empty
            } else {
                if detail { Hairline() }
                ForEach(Array(shown.enumerated()), id: \.element.id) { index, delivery in
                    if index > 0 { Hairline().padding(.leading, 46) }
                    row(delivery)
                        .padding(.vertical, detail ? 6 : (shown.count == rows ? 2 : 6))
                }
                if detail { delivered(room: rows - shown.count) }
            }
            Spacer(minLength: 0)
        }
    }

    private func header(count: Int) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(count == 0 ? "Deliveries" : "On the way")
                .font(OmniFont.sans(15, .semibold, relativeTo: .headline))
                .foregroundStyle(Omni.text)
            if count > 0 {
                Text("\(count)")
                    .font(OmniFont.mono(13, .regular, relativeTo: .subheadline))
                    .foregroundStyle(Omni.text3)
            }
            Spacer(minLength: 8)
            if entry.isStale {
                StaleStamp(fetchedAt: entry.fetchedAt, isStale: true)
            } else if let fetched = snapshot.fetchedAt.map(dateFromMs) {
                Text("updated \(OmniFormat.ago(fetched, now: entry.date))")
                    .font(OmniFont.mono(11, .regular, relativeTo: .caption2))
                    .foregroundStyle(Omni.text3)
            }
        }
        .lineLimit(1)
    }

    private func row(_ delivery: Delivery) -> some View {
        HStack(alignment: .center, spacing: 12) {
            DeliveryGlyph(status: delivery.status, pipWidth: 6)
                .frame(width: 34, alignment: .leading)
            VStack(alignment: .leading, spacing: 2) {
                Text(delivery.displayTitle)
                    .font(OmniFont.sans(14, .semibold, relativeTo: .subheadline))
                    .foregroundStyle(Omni.text)
                    .lineLimit(1)
                HStack(spacing: 5) {
                    DeliveryStatusText(delivery: delivery)
                    if let carrier = delivery.carrier {
                        Text("· \(carrier)")
                            .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                            .foregroundStyle(Omni.text3)
                            .lineLimit(1)
                    }
                }
                if detail, let event = eventLine(delivery) {
                    Text(event)
                        .font(OmniFont.sans(12, .regular, relativeTo: .caption))
                        .foregroundStyle(Omni.text3)
                        .lineLimit(1)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            ExpectedText(delivery: delivery, now: entry.date, size: 13)
                .fixedSize()
        }
        .accessibilityElement(children: .combine)
    }

    /// Last event and place, skipping an event that only repeats the status.
    private func eventLine(_ delivery: Delivery) -> String? {
        let event = delivery.lastEvent?.caseInsensitiveCompare(delivery.statusLabel) == .orderedSame
            ? nil : delivery.lastEvent
        let parts = [event, delivery.lastLocation].compactMap { $0?.isEmpty == false ? $0 : nil }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    @ViewBuilder private func delivered(room: Int) -> some View {
        let done = snapshot.completed.prefix(max(0, min(room, 2)))
        if !done.isEmpty {
            Text("Delivered")
                .font(OmniFont.sans(12, .medium, relativeTo: .caption))
                .foregroundStyle(Omni.text3)
                .padding(.top, 12)
                .padding(.bottom, 4)
            ForEach(Array(done)) { delivery in
                HStack(spacing: 12) {
                    DeliveryGlyph(status: .completed, pipWidth: 6).frame(width: 34, alignment: .leading)
                    Text(delivery.displayTitle)
                        .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                        .foregroundStyle(Omni.text2)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if let at = DeliveryDates.parse(delivery.lastEventAt)?.date {
                        Text(OmniFormat.ago(at, now: entry.date))
                            .font(OmniFont.mono(11, .regular, relativeTo: .caption2))
                            .foregroundStyle(Omni.text3)
                    }
                }
                .padding(.vertical, 3)
            }
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 4) {
            Spacer(minLength: 0)
            Text("Nothing on the way")
                .font(OmniFont.sans(17, .semibold, relativeTo: .headline))
                .foregroundStyle(Omni.text)
            if let last = snapshot.completed.first {
                Text("Last: \(last.displayTitle) delivered")
                    .font(OmniFont.sans(13, .regular, relativeTo: .footnote))
                    .foregroundStyle(Omni.text3)
                    .lineLimit(1)
            }
        }
    }
}

struct DeliveriesRectangular: View {
    let entry: DeliveriesEntry
    let snapshot: DeliveriesSnapshot

    var body: some View {
        let ranked = DeliveryOrdering.ranked(snapshot.active, now: entry.date)
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 5) {
                Image(systemName: "shippingbox.fill").font(.system(size: 11, weight: .semibold))
                Text(ranked.isEmpty ? "Nothing on the way" : "\(ranked.count) on the way")
                    .font(OmniFont.sans(14, .semibold, relativeTo: .headline))
            }
            .widgetAccentable()
            ForEach(ranked.prefix(2)) { delivery in
                HStack(spacing: 6) {
                    Text(delivery.displayTitle)
                        .font(OmniFont.sans(14, .regular, relativeTo: .subheadline))
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    Text(lockLabel(delivery))
                        .font(OmniFont.mono(12, .medium, relativeTo: .subheadline))
                        .lineLimit(1)
                        .fixedSize()
                }
            }
            if ranked.isEmpty, let last = snapshot.completed.first {
                Text("\(last.displayTitle) delivered")
                    .font(OmniFont.sans(13, .regular, relativeTo: .subheadline))
                    .lineLimit(1)
                    .opacity(0.7)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// Lock Screen width is tight: "Late" instead of "Was due Oct 10".
    private func lockLabel(_ delivery: Delivery) -> String {
        if DeliveryDates.isOverdue(expected: delivery.expected, expectedEnd: delivery.expectedEnd, now: entry.date) {
            return "Late"
        }
        return DeliveryDates.friendlyExpected(
            expected: delivery.expected, expectedEnd: delivery.expectedEnd, now: entry.date
        ) ?? delivery.statusLabel
    }
}
