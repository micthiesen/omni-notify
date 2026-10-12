import SafariServices
import SwiftUI
import WidgetKit

/// App-wide state: the web shell, the settings sheet and URL routing.
@MainActor
final class AppModel: ObservableObject {
    static let shared = AppModel()

    let web: WebShell
    @Published var showsSettings = false
    /// Last resort when no app or Safari would take a link: an in-app Safari sheet.
    @Published var safariURL: IdentifiedURL?

    private let settings = OmniSettings.shared
    private var isActive = false
    /// Routes that arrive before the scene is active (cold launch from a widget or a
    /// notification) wait here; `UIApplication.open` is unreliable until then.
    private var pending: [(target: URL, appURL: URL?)] = []

    init() {
        web = WebShell(home: OmniSettings.shared.serverURL ?? AppModel.fallbackHome)
        if !isConfigured { showsSettings = true }
        web.onSettingsGesture = { [weak self] in self?.showsSettings = true }
        web.openExternally = { [weak self] url in self?.openExternally([url]) }
        #if DEBUG
            // Headless check of deep-link routing: `simctl launch <id> <bundle> -OmniOpenURL omni://open?...`
            if let raw = UserDefaults.standard.string(forKey: "OmniOpenURL"), let url = URL(string: raw) {
                handle(url: url)
            }
        #endif
    }

    static let fallbackHome = URL(string: "http://omni.boris")!

    var isConfigured: Bool {
        settings.serverURL != nil && settings.authToken.count >= 24
    }

    var home: URL { settings.serverURL ?? Self.fallbackHome }

    /// `omni://open?u=...&a=...` from widgets: forward without showing the web view.
    func handle(url: URL) {
        guard let open = OmniDeepLink.parse(url) else { return }
        route(target: open.target, appURL: open.appURL)
    }

    func handleNotification(_ payload: OmniPushPayload?) {
        guard let target = payload?.targetURL else { return }
        route(target: target, appURL: nil)
    }

    func route(target: URL, appURL: URL?) {
        guard isActive else {
            pending.append((target, appURL))
            return
        }
        switch OmniRoute.decide(target: target, appURL: appURL, server: home) {
        case let .webView(url):
            showsSettings = false
            web.load(url)
        case let .external(candidates):
            openExternally(candidates)
        }
    }

    /// Tries each URL in order (app scheme, then the universal https link). If iOS
    /// refuses all of them, a web URL still opens in an in-app Safari sheet.
    func openExternally(_ candidates: [URL]) {
        guard let first = candidates.first else { return }
        let rest = Array(candidates.dropFirst())
        UIApplication.shared.open(first) { opened in
            guard !opened else { return }
            Task { @MainActor in
                if rest.isEmpty {
                    self.presentInSafari(first)
                } else {
                    self.openExternally(rest)
                }
            }
        }
    }

    private func presentInSafari(_ url: URL) {
        guard let scheme = url.scheme?.lowercased(), ["http", "https"].contains(scheme) else { return }
        showsSettings = false
        safariURL = IdentifiedURL(url: url)
    }

    func serverChanged() {
        web.setHome(home)
    }

    func becameActive() {
        isActive = true
        let queued = pending
        pending.removeAll()
        for item in queued {
            route(target: item.target, appURL: item.appURL)
        }
        web.recoverIfNeeded()
        Task { await PushRegistrar.shared.foreground() }
        guard isConfigured else { return }
        // Opening the app retries control registrations the extension could not finish.
        Task.detached { _ = try? await ControlRegistrationSync.syncCurrentControls() }
    }

    func resignedActive() {
        isActive = false
    }
}

struct IdentifiedURL: Identifiable {
    let url: URL
    var id: String { url.absoluteString }
}

struct SafariView: UIViewControllerRepresentable {
    let url: URL

    func makeUIViewController(context _: Context) -> SFSafariViewController {
        SFSafariViewController(url: url)
    }

    func updateUIViewController(_: SFSafariViewController, context _: Context) {}
}
