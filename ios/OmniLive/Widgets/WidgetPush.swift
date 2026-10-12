import Foundation
import WidgetKit

/// Receives the WidgetKit push token (iOS 26) and registers it with Omni, which sends
/// `content-changed` pushes when the on-air set, order or titles change.
struct OmniWidgetPushHandler: WidgetPushHandler {
    init() {}

    func pushTokenDidChange(_ pushInfo: WidgetPushInfo, widgets _: [WidgetInfo]) {
        let settings = OmniSettings.shared
        let registration = WidgetPushRegistration(
            deviceId: settings.deviceId,
            pushToken: pushInfo.token.hexString,
            environment: settings.apnsEnvironment
        )
        // Persisted before sending; the app retries anything left pending.
        Task { await DeviceRegistrationSync.shared.syncWidgetPush(registration) }
    }
}
