import SwiftUI
import WidgetKit

struct DeliveriesEntry: TimelineEntry {
    let date: Date
    let snapshot: DeliveriesSnapshot?
    let fetchedAt: Date?
    let isStale: Bool
    let problem: WidgetProblem?

    var relevance: TimelineEntryRelevance? {
        let urgent = snapshot?.active.contains { $0.status == .outForDelivery } ?? false
        return TimelineEntryRelevance(score: urgent ? 0.8 : (snapshot?.activeCount ?? 0) > 0 ? 0.1 : 0)
    }

    static func fixture(_ snapshot: DeliveriesSnapshot, stale: Bool = false) -> DeliveriesEntry {
        DeliveriesEntry(
            date: Date(),
            snapshot: snapshot,
            fetchedAt: stale ? Date().addingTimeInterval(-3 * 3_600) : Date(),
            isStale: stale,
            problem: nil
        )
    }
}

struct DeliveriesProvider: TimelineProvider {
    static let cacheName = "deliveries"

    func placeholder(in _: Context) -> DeliveriesEntry {
        .fixture(WidgetFixtures.deliveries())
    }

    func getSnapshot(in context: Context, completion: @escaping (DeliveriesEntry) -> Void) {
        if context.isPreview {
            completion(.fixture(WidgetFixtures.deliveries()))
            return
        }
        let done = UncheckedCompletion(call: completion)
        Task { done.call(await Self.entry()) }
    }

    func getTimeline(in _: Context, completion: @escaping (Timeline<DeliveriesEntry>) -> Void) {
        let done = UncheckedCompletion(call: completion)
        Task {
            let entry = await Self.entry()
            let next = Date().addingTimeInterval(DeliveriesPolicy.refreshInterval(for: entry.snapshot))
            done.call(Timeline(entries: [entry], policy: .after(next)))
        }
    }

    static func entry() async -> DeliveriesEntry {
        let (cached, error) = await SnapshotStore.resolve(cacheName, maxAge: DeliveriesPolicy.hardExpiry) { try await OmniAPIClient().deliveries() }
        let now = Date()
        let problem: WidgetProblem? = switch error {
        case nil: nil
        case OmniAPIError.notConfigured?: .notConfigured
        default: .unreachable
        }
        let stale = cached.map { now.timeIntervalSince($0.fetchedAt) > DeliveriesPolicy.staleAfter } ?? false
        return DeliveriesEntry(
            date: now,
            snapshot: cached?.value,
            fetchedAt: cached?.fetchedAt,
            isStale: stale,
            problem: problem
        )
    }
}

struct DeliveriesWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: OmniWidgetKind.deliveries, provider: DeliveriesProvider()) { entry in
            DeliveriesWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Deliveries")
        .description("Parcels on the way and when they arrive.")
        .supportedFamilies([.systemSmall, .systemMedium, .systemLarge, .accessoryRectangular])
        .pushHandler(OmniWidgetPushHandler.self)
    }
}

struct DeliveriesWidgetEntryView: View {
    @Environment(\.widgetFamily) private var family
    let entry: DeliveriesEntry

    var body: some View {
        DeliveriesView(entry: entry, family: family)
    }
}
