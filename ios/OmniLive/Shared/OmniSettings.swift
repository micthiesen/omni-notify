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
        let raw = Bundle.main.object(forInfoDictionaryKey: "OmniAPNSEnvironment") as? String
        return raw == "production" ? "production" : "sandbox"
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
