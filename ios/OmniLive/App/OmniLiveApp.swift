import SwiftUI
import UserNotifications

@main
struct OmniLiveApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView(model: AppModel.shared, push: PushRegistrar.shared)
                .preferredColorScheme(.dark)
                .onOpenURL { AppModel.shared.handle(url: $0) }
                .onChange(of: scenePhase, initial: true) { _, phase in
                    if phase == .active {
                        AppModel.shared.becameActive()
                    } else {
                        AppModel.shared.resignedActive()
                    }
                }
        }
    }
}

final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    func application(
        _: UIApplication,
        didFinishLaunchingWithOptions _: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        center.setNotificationCategories(NotificationCategories.all())
        Task { await PushRegistrar.shared.launch() }
        return true
    }

    func application(_: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        PushRegistrar.shared.didRegister(token: deviceToken)
    }

    func application(_: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        PushRegistrar.shared.didFailToRegister(error)
    }

    /// Alerts still show while Omni is open.
    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        willPresent _: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    /// A tap (or the Open action) forwards to the push's URL; otherwise Omni shows.
    nonisolated func userNotificationCenter(
        _: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        guard response.actionIdentifier != UNNotificationDismissActionIdentifier else { return }
        let payload = OmniPushPayload.parse(userInfo: response.notification.request.content.userInfo)
        await MainActor.run { AppModel.shared.handleNotification(payload) }
    }
}

enum NotificationCategories {
    static func all() -> Set<UNNotificationCategory> {
        var categories = Set(OmniNotificationCategory.linkVariants.map { variant in
            UNNotificationCategory(
                identifier: variant.identifier,
                actions: [
                    UNNotificationAction(
                        identifier: OmniNotificationCategory.openAction,
                        title: variant.title,
                        options: [.foreground]
                    ),
                ],
                intentIdentifiers: [],
                options: []
            )
        })
        categories.insert(UNNotificationCategory(
            identifier: OmniNotificationCategory.plain,
            actions: [],
            intentIdentifiers: [],
            options: []
        ))
        return categories
    }
}
