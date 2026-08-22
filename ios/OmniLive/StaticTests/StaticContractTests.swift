import Foundation

private enum FixtureError: Error {
    case offline
}

@main
struct StaticContractTests {
    static func main() async throws {
        let fresh = LiveSlotState.preview(slot: 1)
        let cached = LiveSlotState(
            slot: 1,
            isLive: true,
            streamerId: "cached",
            displayName: "Cached Channel",
            title: "Cached title",
            platform: "youtube",
            url: URL(string: "https://youtube.com/@cached/live")!,
            viewerCount: 50,
            startedAt: 1,
            updatedAt: 2
        )
        let home = URL(string: "http://omni.boris")!

        let freshResult = try await LiveSlotResolution.currentState(
            fetch: { fresh },
            cached: { cached }
        )
        precondition(freshResult == fresh, "Fresh state must win over cache")

        let cachedResult = try await LiveSlotResolution.currentState(
            fetch: { throw FixtureError.offline },
            cached: { cached }
        )
        precondition(cachedResult == cached, "Cache must cover a failed refresh")

        let cachedTap = await LiveSlotResolution.tapURL(
            fetch: { throw FixtureError.offline },
            cached: { cached },
            homeURL: home
        )
        precondition(cachedTap == cached.url, "Cached stream must remain tappable")

        let homeTap = await LiveSlotResolution.tapURL(
            fetch: { throw FixtureError.offline },
            cached: { nil },
            homeURL: home
        )
        precondition(homeTap == home, "No-cache failure must open Omni home")

        let wireData = try JSONEncoder().encode(fresh)
        let decoded = try JSONDecoder().decode(LiveSlotState.self, from: wireData)
        precondition(decoded == fresh, "Live slot wire model must round-trip")

        let diagnosticsData = Data(#"""
        {"apnsEnabled":true,"registrationCount":4,"undeliveredCount":0,"lastReconciledAt":123}
        """#.utf8)
        let diagnostics = try JSONDecoder().decode(
            ControlServerDiagnostics.self,
            from: diagnosticsData
        )
        precondition(diagnostics.apnsEnabled, "APNs diagnostic must decode")
        precondition(diagnostics.registrationCount == 4, "Registration count must decode")
        precondition(diagnostics.undeliveredCount == 0, "Delivery state must decode")

        print("Static control fixtures passed: fresh, cached, tap, home, wire, and diagnostics")
    }
}
