import SwiftUI

struct RootView: View {
    @ObservedObject var model: AppModel
    @ObservedObject var push: PushRegistrar
    @ObservedObject private var web: WebShell

    init(model: AppModel, push: PushRegistrar) {
        self.model = model
        self.push = push
        web = model.web
    }

    var body: some View {
        ZStack(alignment: .top) {
            Omni.canvas.ignoresSafeArea()
            WebViewContainer(shell: web)
                .ignoresSafeArea()
                .opacity(isFailed ? 0 : 1)
            if case let .failed(message) = web.phase {
                OfflineView(
                    host: model.home.host() ?? "Omni",
                    detail: message,
                    retry: { web.retry() },
                    openSettings: { model.showsSettings = true }
                )
                .transition(.opacity)
            }
            LoadBar(progress: web.progress, visible: web.progress < 1 && !isFailed)
            SettingsHandle { model.showsSettings = true }
        }
        .sheet(isPresented: $model.showsSettings) {
            SettingsView(push: push) {
                model.showsSettings = false
            } serverChanged: {
                model.serverChanged()
            }
            .presentationDragIndicator(.visible)
            .preferredColorScheme(.dark)
        }
        .sheet(item: $model.safariURL) { item in
            SafariView(url: item.url).ignoresSafeArea()
        }
        .animation(.easeOut(duration: 0.18), value: isFailed)
    }

    private var isFailed: Bool {
        if case .failed = web.phase { return true }
        return false
    }
}

/// A 2 pt Signal line along the top edge while a page loads.
private struct LoadBar: View {
    let progress: Double
    let visible: Bool

    var body: some View {
        GeometryReader { proxy in
            Rectangle()
                .fill(Omni.signal)
                .frame(width: proxy.size.width * max(0.04, progress), height: 2)
                .opacity(visible ? 1 : 0)
                .animation(.easeOut(duration: 0.2), value: progress)
        }
        .frame(height: 2)
        .allowsHitTesting(false)
    }
}

/// The way into native settings: a slim pill on the trailing edge, clear of the web
/// UI's top bar and tab bar on iPhone and iPad alike.
private struct SettingsHandle: View {
    let action: () -> Void

    var body: some View {
        GeometryReader { proxy in
            Button(action: action) {
                Capsule()
                    .fill(Color.white.opacity(0.22))
                    .frame(width: 4, height: 40)
                    .frame(width: 18, height: 64, alignment: .trailing)
                    .padding(.trailing, 3)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Omni app settings")
            .position(x: proxy.size.width - 12, y: proxy.size.height * 0.4)
        }
    }
}

/// Instrument ErrorState for the whole page: what failed, why, Retry.
struct OfflineView: View {
    let host: String
    let detail: String
    let retry: () -> Void
    let openSettings: () -> Void

    @State private var showsDetail = false

    var body: some View {
        ZStack {
            Omni.canvas.ignoresSafeArea()
            VStack(alignment: .leading, spacing: 0) {
                ZStack {
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(Omni.raised)
                        .overlay(
                            RoundedRectangle(cornerRadius: 12, style: .continuous)
                                .strokeBorder(Omni.line, lineWidth: 1)
                        )
                    Diamond().fill(Omni.warn).frame(width: 12, height: 12)
                }
                .frame(width: 40, height: 40)
                Text("Can't reach Omni")
                    .font(OmniFont.sans(26, .semibold, relativeTo: .title))
                    .tracking(-0.5)
                    .foregroundStyle(Omni.text)
                    .padding(.top, 20)
                Text("\(host) didn't answer. Omni is reachable at home or over WireGuard.")
                    .font(OmniFont.sans(15, .regular, relativeTo: .body))
                    .foregroundStyle(Omni.text2)
                    .padding(.top, 8)
                HStack(spacing: 12) {
                    Button(action: retry) {
                        Text("Try again")
                            .font(OmniFont.sans(15, .semibold, relativeTo: .body))
                            .foregroundStyle(Omni.signalInk)
                            .padding(.horizontal, 20)
                            .frame(height: 44)
                            .background(Omni.signal, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                    }
                    Button(action: openSettings) {
                        Text("Settings")
                            .font(OmniFont.sans(15, .medium, relativeTo: .body))
                            .foregroundStyle(Omni.text)
                            .padding(.horizontal, 20)
                            .frame(height: 44)
                            .background(Omni.raised, in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                            .overlay(
                                RoundedRectangle(cornerRadius: 8, style: .continuous)
                                    .strokeBorder(Omni.lineStrong, lineWidth: 1)
                            )
                    }
                }
                .buttonStyle(.plain)
                .padding(.top, 24)
                DisclosureGroup(isExpanded: $showsDetail) {
                    Text(detail)
                        .font(OmniFont.mono(13, .regular, relativeTo: .footnote))
                        .foregroundStyle(Omni.text3)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 6)
                } label: {
                    Text("Details")
                        .font(OmniFont.sans(13, .medium, relativeTo: .footnote))
                        .foregroundStyle(Omni.text3)
                }
                .tint(Omni.text3)
                .padding(.top, 24)
            }
            .frame(maxWidth: 440, alignment: .leading)
            .padding(24)
        }
    }
}
