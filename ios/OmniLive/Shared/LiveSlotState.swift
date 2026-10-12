import Foundation

struct LiveSlotState: Codable, Equatable, Sendable {
    let slot: Int
    let isLive: Bool
    let streamerId: String?
    let displayName: String
    let title: String?
    let platform: String?
    let url: URL
    let viewerCount: Int?
    let startedAt: Double?
    let updatedAt: Double

    static func preview(slot: Int = 1) -> LiveSlotState {
        LiveSlotState(
            slot: slot,
            isLive: true,
            streamerId: "sample",
            displayName: "Sample Channel",
            title: "A live stream from Omni",
            platform: "twitch",
            url: URL(string: "https://twitch.tv")!,
            viewerCount: 1_234,
            startedAt: Date().addingTimeInterval(-3_600).timeIntervalSince1970 * 1_000,
            updatedAt: Date().timeIntervalSince1970 * 1_000
        )
    }
}

struct ControlRegistration: Codable, Equatable, Sendable {
    let controlId: String
    let slot: Int
    let pushToken: String
    let environment: String
}

struct ControlServerDiagnostics: Codable, Equatable, Sendable {
    let apnsEnabled: Bool
    let registrationCount: Int
    let undeliveredCount: Int
    let lastReconciledAt: Double?
}

struct ControlRegistrationRequest: Codable, Sendable {
    let deviceId: String
    let controls: [ControlRegistration]
}

extension Data {
    /// Lowercase hex, the wire form of APNs and WidgetKit push tokens.
    var hexString: String { map { String(format: "%02x", $0) }.joined() }
}
