import XCTest
@testable import OmniLive

final class LiveSlotStateTests: XCTestCase {
    func testDecodesServerSlotContract() throws {
        let data = Data(#"""
        {
          "slot": 2,
          "isLive": true,
          "streamerId": "channel",
          "displayName": "Channel",
          "title": "Live now",
          "platform": "youtube",
          "url": "https://youtube.com/@channel/live",
          "viewerCount": 42,
          "startedAt": 1787054400000,
          "updatedAt": 1787058000000
        }
        """#.utf8)

        let state = try JSONDecoder().decode(LiveSlotState.self, from: data)
        XCTAssertEqual(state.slot, 2)
        XCTAssertEqual(state.displayName, "Channel")
        XCTAssertEqual(state.viewerCount, 42)
        XCTAssertEqual(state.url.absoluteString, "https://youtube.com/@channel/live")
    }

    func testRegistrationEncodesExpectedWireNames() throws {
        let registration = ControlRegistration(
            controlId: "control-1",
            slot: 4,
            pushToken: String(repeating: "ab", count: 32),
            environment: "sandbox"
        )

        let object = try XCTUnwrap(
            JSONSerialization.jsonObject(with: JSONEncoder().encode(registration)) as? [String: Any]
        )
        XCTAssertEqual(object["controlId"] as? String, "control-1")
        XCTAssertEqual(object["slot"] as? Int, 4)
        XCTAssertEqual(object["environment"] as? String, "sandbox")
    }

    func testFreshStateWinsOverCache() async throws {
        let fresh = LiveSlotState.preview(slot: 2)
        let cached = LiveSlotState.preview(slot: 3)
        let resolved = try await LiveSlotResolution.currentState(
            fetch: { fresh },
            cached: { cached }
        )
        XCTAssertEqual(resolved.slot, 2)
    }

    func testCurrentStateFallsBackToCache() async throws {
        let cached = LiveSlotState.preview(slot: 3)
        let resolved = try await LiveSlotResolution.currentState(
            fetch: { throw URLError(.notConnectedToInternet) },
            cached: { cached }
        )
        XCTAssertEqual(resolved.slot, 3)
    }

    func testTapFallsBackToHomeWithoutCache() async {
        let home = URL(string: "http://omni.boris")!
        let resolved = await LiveSlotResolution.tapURL(
            fetch: { throw URLError(.timedOut) },
            cached: { nil },
            homeURL: home
        )
        XCTAssertEqual(resolved, home)
    }
}
