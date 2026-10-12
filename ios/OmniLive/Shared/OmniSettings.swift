import Foundation

final class OmniSettings: @unchecked Sendable {
    static let shared = OmniSettings()

    private enum Key {
        static let serverURL = "serverURL"
        static let authToken = "authToken"
        static let deviceId = "deviceId"
        static let lastRegistrationAt = "lastRegistrationAt"
        static let lastRegistrationCount = "lastRegistrationCount"
        static let pendingRegistrations = "pendingRegistrations"
        static let lastRegistrationError = "lastRegistrationError"
        static let alertsEnabled = "alertsEnabled"
        static let apnsToken = "apnsToken"
        static let pendingDevice = "pendingDevice"
        static let lastSentDevice = "lastSentDevice"
        static let lastDeviceRegistrationAt = "lastDeviceRegistrationAt"
        static let deviceRegistrationError = "deviceRegistrationError"
        static let serverApnsEnabled = "serverApnsEnabled"
        static let pendingWidgetPush = "pendingWidgetPush"
        static let lastSentWidgetPush = "lastSentWidgetPush"
        static let widgetPushError = "widgetPushError"
        static let lastWidgetPushAt = "lastWidgetPushAt"
        static let didRequestNotificationAuthorization = "didRequestNotificationAuthorization"
        static func cachedSlot(_ slot: Int) -> String { "cachedSlot.\(slot)" }
    }

    private let defaults: UserDefaults
    private let lock = NSLock()

    private init(bundle: Bundle = .main) {
        let suite = bundle.object(forInfoDictionaryKey: "OmniAppGroup") as? String
        defaults = suite.flatMap(UserDefaults.init(suiteName:)) ?? .standard
        if defaults.string(forKey: Key.serverURL) == nil,
           let initial = bundle.object(forInfoDictionaryKey: "OmniDefaultServerURL") as? String,
           !initial.isEmpty
        {
            defaults.set(initial, forKey: Key.serverURL)
        }
        if defaults.string(forKey: Key.authToken) == nil,
           let initial = bundle.object(forInfoDictionaryKey: "OmniDefaultAuthToken") as? String,
           !initial.isEmpty
        {
            defaults.set(initial, forKey: Key.authToken)
        }
    }

    var serverURLString: String {
        get { locked { defaults.string(forKey: Key.serverURL) ?? "" } }
        set { locked { defaults.set(newValue.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.serverURL) } }
    }

    var serverURL: URL? {
        guard let url = URL(string: serverURLString),
              let scheme = url.scheme,
              ["http", "https"].contains(scheme),
              url.host != nil
        else { return nil }
        return url
    }

    var authToken: String {
        get { locked { defaults.string(forKey: Key.authToken) ?? "" } }
        set { locked { defaults.set(newValue.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.authToken) } }
    }

    var deviceId: String {
        locked {
            if let existing = defaults.string(forKey: Key.deviceId) { return existing }
            let created = UUID().uuidString.lowercased()
            defaults.set(created, forKey: Key.deviceId)
            return created
        }
    }

    var apnsEnvironment: String {
        Self.apnsEnvironment(entitlement: Bundle.main.object(forInfoDictionaryKey: "OmniAPNSEnvironment") as? String)
    }

    /// `aps-environment` (development / production) to Omni's wire value.
    static func apnsEnvironment(entitlement: String?) -> String {
        entitlement == "production" ? "production" : "sandbox"
    }

    var lastRegistrationAt: Date? {
        get { locked { defaults.object(forKey: Key.lastRegistrationAt) as? Date } }
        set { locked { defaults.set(newValue, forKey: Key.lastRegistrationAt) } }
    }

    var lastRegistrationCount: Int {
        get { locked { defaults.integer(forKey: Key.lastRegistrationCount) } }
        set { locked { defaults.set(newValue, forKey: Key.lastRegistrationCount) } }
    }

    var pendingRegistrations: [ControlRegistration]? {
        get {
            locked {
                guard let data = defaults.data(forKey: Key.pendingRegistrations) else { return nil }
                return try? JSONDecoder().decode([ControlRegistration].self, from: data)
            }
        }
        set {
            locked {
                if let newValue, let data = try? JSONEncoder().encode(newValue) {
                    defaults.set(data, forKey: Key.pendingRegistrations)
                } else {
                    defaults.removeObject(forKey: Key.pendingRegistrations)
                }
            }
        }
    }

    var lastRegistrationError: String? {
        get { locked { defaults.string(forKey: Key.lastRegistrationError) } }
        set { locked { defaults.set(newValue, forKey: Key.lastRegistrationError) } }
    }

    /// This device's alert toggle; nil until the user changes it (then the device
    /// kind's default applies).
    var alertsEnabledOverride: Bool? {
        get { locked { defaults.object(forKey: Key.alertsEnabled) as? Bool } }
        set { locked { defaults.set(newValue, forKey: Key.alertsEnabled) } }
    }

    func alertsEnabled(for kind: DeviceKind) -> Bool {
        alertsEnabledOverride ?? kind.alertsEnabledByDefault
    }

    /// Hex APNs device token from the last successful remote-notification registration.
    var apnsToken: String? {
        get { locked { defaults.string(forKey: Key.apnsToken) } }
        set { locked { defaults.set(newValue, forKey: Key.apnsToken) } }
    }

    var pendingDevice: DeviceRegistration? {
        get { codable(Key.pendingDevice) }
        set { setCodable(newValue, Key.pendingDevice) }
    }

    var lastSentDevice: DeviceRegistration? {
        get { codable(Key.lastSentDevice) }
        set { setCodable(newValue, Key.lastSentDevice) }
    }

    var lastDeviceRegistrationAt: Date? {
        get { locked { defaults.object(forKey: Key.lastDeviceRegistrationAt) as? Date } }
        set { locked { defaults.set(newValue, forKey: Key.lastDeviceRegistrationAt) } }
    }

    var deviceRegistrationError: String? {
        get { locked { defaults.string(forKey: Key.deviceRegistrationError) } }
        set { locked { defaults.set(newValue, forKey: Key.deviceRegistrationError) } }
    }

    var serverApnsEnabled: Bool? {
        get { locked { defaults.object(forKey: Key.serverApnsEnabled) as? Bool } }
        set { locked { defaults.set(newValue, forKey: Key.serverApnsEnabled) } }
    }

    var pendingWidgetPush: WidgetPushRegistration? {
        get { codable(Key.pendingWidgetPush) }
        set { setCodable(newValue, Key.pendingWidgetPush) }
    }

    var lastSentWidgetPush: WidgetPushRegistration? {
        get { codable(Key.lastSentWidgetPush) }
        set { setCodable(newValue, Key.lastSentWidgetPush) }
    }

    var lastWidgetPushAt: Date? {
        get { locked { defaults.object(forKey: Key.lastWidgetPushAt) as? Date } }
        set { locked { defaults.set(newValue, forKey: Key.lastWidgetPushAt) } }
    }

    var widgetPushError: String? {
        get { locked { defaults.string(forKey: Key.widgetPushError) } }
        set { locked { defaults.set(newValue, forKey: Key.widgetPushError) } }
    }

    var didRequestNotificationAuthorization: Bool {
        get { locked { defaults.bool(forKey: Key.didRequestNotificationAuthorization) } }
        set { locked { defaults.set(newValue, forKey: Key.didRequestNotificationAuthorization) } }
    }

    /// Shared container for widget caches (snapshots, avatars). Falls back to the
    /// process caches directory where no App Group exists (tests, unsigned builds).
    var sharedContainerURL: URL {
        if let group = Bundle.main.object(forInfoDictionaryKey: "OmniAppGroup") as? String,
           !group.isEmpty,
           let url = FileManager.default.containerURL(
               forSecurityApplicationGroupIdentifier: group
           )
        {
            return url
        }
        return FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
    }

    private func codable<T: Decodable>(_ key: String) -> T? {
        locked {
            guard let data = defaults.data(forKey: key) else { return nil }
            return try? JSONDecoder().decode(T.self, from: data)
        }
    }

    private func setCodable<T: Encodable>(_ value: T?, _ key: String) {
        locked {
            if let value, let data = try? JSONEncoder().encode(value) {
                defaults.set(data, forKey: key)
            } else {
                defaults.removeObject(forKey: key)
            }
        }
    }

    func cache(_ state: LiveSlotState) {
        guard let data = try? JSONEncoder().encode(state) else { return }
        locked { defaults.set(data, forKey: Key.cachedSlot(state.slot)) }
    }

    func cachedSlot(_ slot: Int) -> LiveSlotState? {
        locked {
            guard let data = defaults.data(forKey: Key.cachedSlot(slot)) else { return nil }
            return try? JSONDecoder().decode(LiveSlotState.self, from: data)
        }
    }

    private func locked<T>(_ operation: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return operation()
    }
}
