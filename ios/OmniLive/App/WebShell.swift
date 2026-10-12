import SwiftUI
import WebKit

/// The one long-lived `WKWebView` showing Omni. It lives in `AppModel`, so its page,
/// scroll position and history survive app switches and settings changes.
@MainActor
final class WebShell: NSObject, ObservableObject {
    enum Phase: Equatable {
        case loading
        case ready
        case failed(String)
    }

    @Published private(set) var phase: Phase = .loading
    @Published private(set) var progress: Double = 0

    let webView: WKWebView
    /// Two-finger long press anywhere: the second way into settings.
    var onSettingsGesture: (() -> Void)?
    /// Hands links that leave Omni to the app router (app, Safari, or in-app Safari).
    var openExternally: ((URL) -> Void)?
    private(set) var home: URL
    private let refresh = UIRefreshControl()
    private var progressObservation: NSKeyValueObservation?

    /// iPhone gets true edge-to-edge: the web phone layout already pads its top bar and
    /// tab bar with `env(safe-area-inset-*)`, which needs `viewport-fit=cover`. The iPad
    /// (desktop layout) keeps automatic safe-area insets.
    private static let coverViewport = """
    (function () {
      var meta = document.querySelector('meta[name="viewport"]');
      if (!meta) { meta = document.createElement('meta'); meta.name = 'viewport'; document.head.appendChild(meta); }
      if (meta.content.indexOf('viewport-fit') < 0) {
        meta.content = (meta.content ? meta.content + ', ' : '') + 'viewport-fit=cover';
      }
    })();
    """

    init(home: URL) {
        self.home = home
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .default()
        configuration.allowsInlineMediaPlayback = true
        configuration.mediaTypesRequiringUserActionForPlayback = []
        let isPhone = UIDevice.current.userInterfaceIdiom == .phone
        if isPhone {
            configuration.userContentController.addUserScript(WKUserScript(
                source: Self.coverViewport,
                injectionTime: .atDocumentEnd,
                forMainFrameOnly: true
            ))
        }
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()

        webView.isOpaque = false
        webView.backgroundColor = Omni.canvasUIColor
        webView.underPageBackgroundColor = Omni.canvasUIColor
        webView.scrollView.backgroundColor = Omni.canvasUIColor
        webView.scrollView.contentInsetAdjustmentBehavior = isPhone ? .never : .automatic
        webView.scrollView.alwaysBounceVertical = true
        webView.allowsBackForwardNavigationGestures = true
        webView.allowsLinkPreview = true
        webView.navigationDelegate = self
        webView.uiDelegate = self
        #if DEBUG
            webView.isInspectable = true
        #endif

        refresh.tintColor = UIColor(white: 0.55, alpha: 1)
        refresh.addTarget(self, action: #selector(pulled), for: .valueChanged)
        webView.scrollView.refreshControl = refresh

        let press = UILongPressGestureRecognizer(target: self, action: #selector(settingsPress(_:)))
        press.numberOfTouchesRequired = 2
        press.minimumPressDuration = 0.6
        press.delegate = self
        webView.addGestureRecognizer(press)

        // WKWebView reports KVO changes on the main thread.
        progressObservation = webView.observe(\.estimatedProgress, options: [.new]) { [weak self] _, change in
            guard let value = change.newValue else { return }
            MainActor.assumeIsolated { self?.progress = value }
        }
        load(home)
    }

    func load(_ url: URL) {
        phase = webView.url == nil ? .loading : phase
        // Omni is on the LAN: if it has not answered in 12 s it is not coming.
        webView.load(URLRequest(url: url, cachePolicy: .useProtocolCachePolicy, timeoutInterval: 12))
    }

    func setHome(_ url: URL) {
        guard url != home else { return }
        home = url
        load(url)
    }

    func retry() {
        phase = .loading
        if let current = webView.url, OmniRoute.sameOrigin(current, home) {
            webView.reload()
        } else {
            load(home)
        }
    }

    /// On every activation: retry a failed load (the phone may be home again or on
    /// WireGuard now), and reload if iOS discarded the page while backgrounded.
    func recoverIfNeeded() {
        if case .failed = phase {
            retry()
        } else if webView.url == nil {
            load(home)
        }
    }

    @objc private func settingsPress(_ recognizer: UILongPressGestureRecognizer) {
        if recognizer.state == .began { onSettingsGesture?() }
    }

    @objc private func pulled() {
        retry()
    }

    fileprivate func finished() {
        phase = .ready
        refresh.endRefreshing()
    }

    fileprivate func failed(_ error: Error) {
        refresh.endRefreshing()
        let nsError = error as NSError
        // A new navigation replaced this one; not a failure.
        if nsError.domain == NSURLErrorDomain, nsError.code == NSURLErrorCancelled { return }
        if nsError.domain == "WebKitErrorDomain", nsError.code == 102 { return }
        phase = .failed(error.localizedDescription)
    }
}

extension WebShell: WKNavigationDelegate, WKUIDelegate {
    func webView(
        _: WKWebView,
        decidePolicyFor action: WKNavigationAction
    ) async -> WKNavigationActionPolicy {
        guard let url = action.request.url, let scheme = url.scheme?.lowercased() else { return .allow }
        switch scheme {
        case "http", "https":
            // Subframes (embedded players) load in place; Omni pages stay in the app;
            // any other site opens in its app or Safari.
            let isMainFrame = action.targetFrame?.isMainFrame ?? true
            if isMainFrame, !OmniRoute.sameOrigin(url, home) {
                openExternally?(url)
                return .cancel
            }
            return .allow
        case "about", "blob", "data", "javascript":
            return .allow
        default:
            // mailto:, tel:, twitch://, ...
            openExternally?(url)
            return .cancel
        }
    }

    /// `target="_blank"` and `window.open`: Omni pages load here, the rest open outside.
    func webView(
        _ webView: WKWebView,
        createWebViewWith _: WKWebViewConfiguration,
        for action: WKNavigationAction,
        windowFeatures _: WKWindowFeatures
    ) -> WKWebView? {
        if let url = action.request.url {
            if OmniRoute.sameOrigin(url, home) {
                webView.load(action.request)
            } else {
                openExternally?(url)
            }
        }
        return nil
    }

    func webView(_: WKWebView, didFinish _: WKNavigation!) {
        finished()
    }

    func webView(_: WKWebView, didFail _: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webView(_: WKWebView, didFailProvisionalNavigation _: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        webView.reload()
    }
}

extension WebShell: UIGestureRecognizerDelegate {
    func gestureRecognizer(
        _: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith _: UIGestureRecognizer
    ) -> Bool {
        true
    }
}

struct WebViewContainer: UIViewRepresentable {
    let shell: WebShell

    func makeUIView(context _: Context) -> WKWebView { shell.webView }
    func updateUIView(_: WKWebView, context _: Context) {}
}
