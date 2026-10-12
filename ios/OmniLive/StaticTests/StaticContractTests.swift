import Foundation

private enum FixtureError: Error {
    case offline
}

@main
struct StaticContractTests {
    static func main() async throws {
        if CommandLine.arguments.contains("--sign") {
            // Shared vector with scripts/ios-live/typecheck.sh's bash signer.
            print(OmniSignature.sign(
                token: "test-token-0123456789abcdef",
                timestamp: "1760000000",
                nonce: "00000000-0000-4000-8000-000000000000",
                method: "PUT",
                path: "/api/ios-controls/device",
                body: Data(#"{"a":1}"#.utf8)
            ))
            return
        }
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

        let watch = URL(string: "https://www.youtube.com/watch?v=a&t=1")!
        let app = URL(string: "twitch://stream/x")!
        let link = OmniDeepLink.parse(OmniDeepLink.openURL(target: watch, appURL: app))
        precondition(link == OmniDeepLink.Open(target: watch, appURL: app), "omni://open must round-trip")

        let registration = DeviceRegistration(
            deviceId: "d", name: "n", kind: .ipad, apnsToken: nil, environment: "sandbox",
            alertsEnabled: false, appVersion: "1.0 (1)"
        )
        let registrationJSON = String(decoding: try JSONEncoder().encode(registration), as: UTF8.self)
        precondition(registrationJSON.contains(#""apnsToken":null"#), "apnsToken must encode as null")

        let onAir = try JSONDecoder().decode(
            OnAirSnapshot.self,
            from: JSONEncoder().encode(WidgetFixtures.liveMany(Date(timeIntervalSince1970: 1_760_000_000)))
        )
        precondition(onAir.live.count == 6 && onAir.hasPrimaryLive, "On Air snapshot must round-trip")
        precondition(OnAirPolicy.refreshInterval(for: onAir) == 15 * 60, "Live refresh is 15 minutes")

        let payload = OmniPushPayload.parse(userInfo: [
            "omni": ["url": "https://kick.com/x", "urlTitle": "Watch on Kick", "refreshWidgets": ["on-air"]],
        ])
        precondition(payload?.widgetKindsToReload == [OmniWidgetKind.onAir], "refreshWidgets maps to kinds")
        precondition(
            OmniNotificationCategory.category(for: payload, current: "") == "OMNI_LINK_KICK",
            "urlTitle picks the titled category"
        )

        print("Static fixtures passed: controls, deep links, registration, On Air, and alert payloads")
    }
}
