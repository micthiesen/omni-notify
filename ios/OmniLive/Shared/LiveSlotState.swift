import Foundation
import CryptoKit

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

private struct RegistrationRequest: Codable, Sendable {
    let deviceId: String
    let controls: [ControlRegistration]
}

enum OmniAPIError: LocalizedError, Equatable {
    case notConfigured
    case invalidResponse
    case http(Int, String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: "Enter the Omni server URL and control token first."
        case .invalidResponse: "Omni returned an invalid response."
        case let .http(status, message): "Omni returned HTTP \(status): \(message)"
        }
    }
}

struct OmniAPIClient: Sendable {
    private let session: URLSession

    init(session: URLSession = .shared) {
        self.session = session
    }

    func liveSlot(_ slot: Int) async throws -> LiveSlotState {
        let settings = OmniSettings.shared
        guard let baseURL = settings.serverURL, !settings.authToken.isEmpty else {
            throw OmniAPIError.notConfigured
        }
        var request = URLRequest(url: baseURL.appending(path: "api/ios-controls/slots/\(slot)"))
        request.timeoutInterval = 8
        authenticate(&request, body: Data(), token: settings.authToken)
        let (data, response) = try await session.data(for: request)
        try validate(response: response, data: data)
        let state = try JSONDecoder().decode(LiveSlotState.self, from: data)
        settings.cache(state)
        return state
    }

    func register(controls: [ControlRegistration]) async throws {
        let settings = OmniSettings.shared
        guard let baseURL = settings.serverURL, !settings.authToken.isEmpty else {
            throw OmniAPIError.notConfigured
        }
        var request = URLRequest(url: baseURL.appending(path: "api/ios-controls/registrations"))
        request.httpMethod = "PUT"
        // Server registration may make two bounded APNs attempts (5s each)
        // before recording a later-tick retry.
        request.timeoutInterval = 15
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body = try JSONEncoder().encode(
            RegistrationRequest(deviceId: settings.deviceId, controls: controls)
        )
        request.httpBody = body
        authenticate(&request, body: body, token: settings.authToken)
        let (data, response) = try await session.data(for: request)
        try validate(response: response, data: data)
        settings.lastRegistrationAt = Date()
        settings.lastRegistrationCount = controls.count
    }

    func diagnostics() async throws -> ControlServerDiagnostics {
        let settings = OmniSettings.shared
        guard let baseURL = settings.serverURL, !settings.authToken.isEmpty else {
            throw OmniAPIError.notConfigured
        }
        var request = URLRequest(url: baseURL.appending(path: "api/ios-controls/diagnostics"))
        request.timeoutInterval = 8
        authenticate(&request, body: Data(), token: settings.authToken)
        let (data, response) = try await session.data(for: request)
        try validate(response: response, data: data)
        return try JSONDecoder().decode(ControlServerDiagnostics.self, from: data)
    }

    private func validate(response: URLResponse, data: Data) throws {
        guard let http = response as? HTTPURLResponse else {
            throw OmniAPIError.invalidResponse
        }
        guard 200 ..< 300 ~= http.statusCode else {
            let message = String(data: data, encoding: .utf8) ?? "Unknown error"
            throw OmniAPIError.http(http.statusCode, message)
        }
    }

    private func authenticate(_ request: inout URLRequest, body: Data, token: String) {
        let timestamp = String(Int(Date().timeIntervalSince1970))
        let nonce = UUID().uuidString.lowercased()
        let bodyHash = SHA256.hash(data: body).map { String(format: "%02x", $0) }.joined()
        let path = request.url?.path(percentEncoded: true) ?? "/"
        let canonical = "\(timestamp)\n\(nonce)\n\(request.httpMethod ?? "GET")\n\(path)\n\(bodyHash)"
        let key = SymmetricKey(data: Data(token.utf8))
        let signature = HMAC<SHA256>.authenticationCode(for: Data(canonical.utf8), using: key)
            .map { String(format: "%02x", $0) }
            .joined()
        request.setValue("Omni-HMAC \(signature)", forHTTPHeaderField: "Authorization")
        request.setValue(timestamp, forHTTPHeaderField: "X-Omni-Timestamp")
        request.setValue(nonce, forHTTPHeaderField: "X-Omni-Nonce")
    }
}
