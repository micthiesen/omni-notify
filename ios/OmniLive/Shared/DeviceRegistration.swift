import Foundation

enum DeviceKind: String, Codable, Sendable {
    case iphone
    case ipad

    /// iPhone receives alerts by default; iPad shows widgets only until toggled.
    var alertsEnabledByDefault: Bool { self == .iphone }
}

/// `PUT /api/ios-controls/device`.
struct DeviceRegistration: Codable, Equatable, Sendable {
    let deviceId: String
    let name: String
    let kind: DeviceKind
    let apnsToken: String?
    let environment: String
    let alertsEnabled: Bool
    let appVersion: String

    private enum CodingKeys: String, CodingKey {
        case deviceId, name, kind, apnsToken, environment, alertsEnabled, appVersion
    }

    init(
        deviceId: String,
        name: String,
        kind: DeviceKind,
        apnsToken: String?,
        environment: String,
        alertsEnabled: Bool,
        appVersion: String
    ) {
        self.deviceId = deviceId
        self.name = DeviceRegistration.truncated(name, utf16Limit: 200)
        self.kind = kind
        self.apnsToken = apnsToken
        self.environment = environment
        self.alertsEnabled = alertsEnabled
        self.appVersion = DeviceRegistration.truncated(appVersion, utf16Limit: 40)
    }

    /// The server bounds strings by UTF-16 length (JS semantics). Cut on a character
    /// boundary so no surrogate pair or grapheme is split.
    static func truncated(_ value: String, utf16Limit: Int) -> String {
        guard value.utf16.count > utf16Limit else { return value }
        var result = ""
        var used = 0
        for character in value {
            let size = character.utf16.count
            guard used + size <= utf16Limit else { break }
            result.append(character)
            used += size
        }
        return result
    }

    /// `apnsToken` is always present on the wire: an explicit `null` keeps the device
    /// registered but un-pushable.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(deviceId, forKey: .deviceId)
        try c.encode(name, forKey: .name)
        try c.encode(kind, forKey: .kind)
        if let apnsToken {
            try c.encode(apnsToken, forKey: .apnsToken)
        } else {
            try c.encodeNil(forKey: .apnsToken)
        }
        try c.encode(environment, forKey: .environment)
        try c.encode(alertsEnabled, forKey: .alertsEnabled)
        try c.encode(appVersion, forKey: .appVersion)
    }

    static func appVersion(info: [String: Any]?) -> String {
        let short = info?["CFBundleShortVersionString"] as? String ?? "0"
        let build = info?["CFBundleVersion"] as? String ?? "0"
        return "\(short) (\(build))"
    }

    static func needsSync(
        current: DeviceRegistration,
        lastSent: DeviceRegistration?,
        lastSentAt: Date?,
        now: Date
    ) -> Bool {
        RegistrationCadence.needsSync(current: current, lastSent: lastSent, lastSentAt: lastSentAt, now: now)
    }
}

struct DeviceRegistrationResponse: Codable, Equatable, Sendable {
    let registered: Bool
    let alertsEnabled: Bool
    let apnsEnabled: Bool
}

/// `PUT /api/ios-controls/widget-push`.
struct WidgetPushRegistration: Codable, Equatable, Sendable {
    let deviceId: String
    let pushToken: String
    let environment: String
}

/// Re-send a registration when it changed, or daily so a token the server dropped
/// (or a lost database row) heals without user action.
enum RegistrationCadence {
    static let refreshInterval: TimeInterval = 24 * 60 * 60

    static func needsSync<T: Equatable>(current: T, lastSent: T?, lastSentAt: Date?, now: Date) -> Bool {
        guard let lastSent, let lastSentAt, lastSent == current else { return true }
        return now.timeIntervalSince(lastSentAt) >= refreshInterval
    }
}

/// Exponential backoff matching the control registration retry: 1, 2, 4, ... seconds.
enum RetryBackoff {
    static func delay(afterAttempt attempt: Int) -> Duration {
        .seconds(1 << min(max(attempt, 0), 5))
    }
}

/// Sends the pending device and widget-push registrations, persisting them in the App
/// Group first so a killed process (or the widget extension) leaves work for the app.
actor DeviceRegistrationSync {
    static let shared = DeviceRegistrationSync()

    private let settings: OmniSettings
    private let client: OmniAPIClient

    init(settings: OmniSettings = .shared, client: OmniAPIClient = OmniAPIClient()) {
        self.settings = settings
        self.client = client
    }

    /// Registers `current` if it differs from what the server last accepted.
    @discardableResult
    func sync(_ current: DeviceRegistration, force: Bool = false, maxAttempts: Int = 4) async
        -> DeviceRegistrationResponse?
    {
        let due = force || settings.pendingDevice != nil || DeviceRegistration.needsSync(
            current: current,
            lastSent: settings.lastSentDevice,
            lastSentAt: settings.lastDeviceRegistrationAt,
            now: Date()
        )
        guard due else { return nil }
        settings.pendingDevice = current
        for attempt in 0 ..< maxAttempts {
            guard let pending = settings.pendingDevice else { return nil }
            do {
                let response = try await client.registerDevice(pending)
                if settings.pendingDevice == pending { settings.pendingDevice = nil }
                settings.lastSentDevice = pending
                settings.lastDeviceRegistrationAt = Date()
                settings.deviceRegistrationError = nil
                settings.serverApnsEnabled = response.apnsEnabled
                return response
            } catch {
                settings.deviceRegistrationError = error.localizedDescription
                guard attempt + 1 < maxAttempts, OmniAPIError.isRetryable(error) else { break }
                try? await Task.sleep(for: RetryBackoff.delay(afterAttempt: attempt))
            }
        }
        return nil
    }

    /// Queues and sends a widget push token. Safe to call from the widget extension.
    func syncWidgetPush(_ registration: WidgetPushRegistration?, maxAttempts: Int = 4) async {
        if let registration {
            let due = settings.pendingWidgetPush != nil || RegistrationCadence.needsSync(
                current: registration,
                lastSent: settings.lastSentWidgetPush,
                lastSentAt: settings.lastWidgetPushAt,
                now: Date()
            )
            guard due else { return }
            settings.pendingWidgetPush = registration
        }
        for attempt in 0 ..< maxAttempts {
            guard let pending = settings.pendingWidgetPush else { return }
            do {
                try await client.registerWidgetPush(pending)
                if settings.pendingWidgetPush == pending { settings.pendingWidgetPush = nil }
                settings.lastSentWidgetPush = pending
                settings.lastWidgetPushAt = Date()
                settings.widgetPushError = nil
                return
            } catch {
                settings.widgetPushError = error.localizedDescription
                guard attempt + 1 < maxAttempts, OmniAPIError.isRetryable(error) else { return }
                try? await Task.sleep(for: RetryBackoff.delay(afterAttempt: attempt))
            }
        }
    }
}
