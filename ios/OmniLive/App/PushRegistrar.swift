import SwiftUI
import UserNotifications
import WidgetKit

/// Notification permission, the APNs device token, and this device's registration.
@MainActor
final class PushRegistrar: ObservableObject {
    static let shared = PushRegistrar()

    @Published private(set) var authorization: UNAuthorizationStatus = .notDetermined
    @Published private(set) var registrationNote = ""
    @Published private(set) var isSyncing = false

    private let settings = OmniSettings.shared

    var kind: DeviceKind {
        UIDevice.current.userInterfaceIdiom == .pad ? .ipad : .iphone
    }

    var alertsEnabled: Bool { settings.alertsEnabled(for: kind) }

    /// iOS only shows alerts the user allowed; telling Omni otherwise would make it skip
    /// the Pushover fallback for alerts nobody would see.
    var canShowAlerts: Bool {
        [.authorized, .provisional, .ephemeral].contains(authorization)
    }

    /// At launch: register with APNs and Omni. The permission prompt waits for the
    /// scene to be active (`foreground()`), where iOS reliably shows it.
    func launch() async {
        await refreshAuthorization()
        UIApplication.shared.registerForRemoteNotifications()
        await sync()
        await syncWidgetPush()
    }

    func foreground() async {
        await refreshAuthorization()
        if kind == .iphone, alertsEnabled, authorization == .notDetermined,
           !settings.didRequestNotificationAuthorization
        {
            await requestAuthorization()
        }
        await sync()
        // Same cadence as the device: re-sent when changed or a day old.
        await syncWidgetPush()
    }

    func requestAuthorization() async {
        do {
            _ = try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
        } catch {
            settings.deviceRegistrationError = "Notifications: \(error.localizedDescription)"
        }
        await refreshAuthorization()
        // Only an answered prompt counts; otherwise the next activation asks again.
        settings.didRequestNotificationAuthorization = authorization != .notDetermined
        UIApplication.shared.registerForRemoteNotifications()
    }

    func refreshAuthorization() async {
        authorization = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    func setAlertsEnabled(_ enabled: Bool) {
        settings.alertsEnabledOverride = enabled
        objectWillChange.send()
        Task {
            if enabled, authorization == .notDetermined { await requestAuthorization() }
            await sync(force: true)
        }
    }

    func didRegister(token: Data) {
        settings.apnsToken = token.hexString
        Task { await sync() }
    }

    func didFailToRegister(_ error: Error) {
        settings.deviceRegistrationError = "APNs: \(error.localizedDescription)"
        objectWillChange.send()
        // Still register (with the last known token, or none) so Omni knows the device.
        Task { await sync() }
    }

    func currentRegistration() -> DeviceRegistration {
        DeviceRegistration(
            deviceId: settings.deviceId,
            name: UIDevice.current.name,
            kind: kind,
            apnsToken: settings.apnsToken,
            environment: settings.apnsEnvironment,
            alertsEnabled: alertsEnabled && canShowAlerts,
            appVersion: DeviceRegistration.appVersion(info: Bundle.main.infoDictionary)
        )
    }

    func sync(force: Bool = false) async {
        guard settings.serverURL != nil, !settings.authToken.isEmpty else { return }
        isSyncing = true
        defer { isSyncing = false }
        await DeviceRegistrationSync.shared.sync(currentRegistration(), force: force)
        objectWillChange.send()
    }

    /// The app can read the widget push token too, which covers an extension that was
    /// killed before it finished registering.
    func syncWidgetPush() async {
        guard settings.serverURL != nil, !settings.authToken.isEmpty else { return }
        let token = await Self.widgetPushToken()
        let registration = token.map {
            WidgetPushRegistration(
                deviceId: settings.deviceId,
                pushToken: $0.hexString,
                environment: settings.apnsEnvironment
            )
        }
        await DeviceRegistrationSync.shared.syncWidgetPush(registration)
        objectWillChange.send()
    }

    private nonisolated static func widgetPushToken() async -> Data? {
        await WidgetCenter.shared.currentPushInfo?.token
    }

    var authorizationLabel: String {
        switch authorization {
        case .authorized: "Allowed"
        case .provisional: "Quiet delivery"
        case .ephemeral: "Temporary"
        case .denied: "Off in Settings"
        case .notDetermined: "Not asked yet"
        @unknown default: "Unknown"
        }
    }
}
