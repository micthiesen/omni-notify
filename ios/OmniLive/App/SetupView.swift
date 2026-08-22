import SwiftUI
import WidgetKit

@MainActor
final class SetupViewModel: ObservableObject {
    @Published var serverURL = OmniSettings.shared.serverURLString
    @Published var authToken = OmniSettings.shared.authToken
    @Published var status = "Ready"
    @Published var slots: [LiveSlotState] = []
    @Published var diagnostics: ControlServerDiagnostics?
    @Published var isWorking = false

    var isConfigured: Bool {
        URL(string: serverURL)?.host != nil && authToken.count >= 24
    }

    func refreshIfConfigured() async {
        guard isConfigured else { return }
        await saveAndTest()
    }

    func saveAndTest() async {
        OmniSettings.shared.serverURLString = serverURL
        OmniSettings.shared.authToken = authToken
        isWorking = true
        defer { isWorking = false }
        do {
            slots = try await withThrowingTaskGroup(of: LiveSlotState.self) { group in
                for slot in 1 ... 4 {
                    group.addTask { try await OmniAPIClient().liveSlot(slot) }
                }
                var result: [LiveSlotState] = []
                for try await value in group { result.append(value) }
                return result.sorted { $0.slot < $1.slot }
            }
            let count = try await ControlRegistrationSync.syncCurrentControls()
            diagnostics = try await OmniAPIClient().diagnostics()
            ControlCenter.shared.reloadAllControls()
            status = "Connected. Synced \(count) configured control\(count == 1 ? "" : "s")."
        } catch {
            status = error.localizedDescription
        }
    }
}

struct SetupView: View {
    @Environment(\.scenePhase) private var scenePhase
    @StateObject private var model = SetupViewModel()

    var body: some View {
        NavigationStack {
            Form {
                Section("Connection") {
                    TextField("Server URL", text: $model.serverURL)
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                    SecureField("Control Token", text: $model.authToken)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button {
                        Task { await model.saveAndTest() }
                    } label: {
                        if model.isWorking {
                            ProgressView()
                        } else {
                            Label("Save, Test, and Sync", systemImage: "arrow.triangle.2.circlepath")
                        }
                    }
                    .disabled(!model.isConfigured || model.isWorking)
                    Text(model.status)
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                Section("Live Slots") {
                    if model.slots.isEmpty {
                        Text("Run the connection test to preview all four Control Center slots.")
                            .foregroundStyle(.secondary)
                    } else {
                        ForEach(model.slots, id: \.slot) { slot in
                            HStack(spacing: 12) {
                                Image(systemName: slot.isLive ? "play.circle.fill" : "moon.zzz")
                                    .foregroundStyle(slot.isLive ? .red : .secondary)
                                VStack(alignment: .leading) {
                                    Text("Slot \(slot.slot): \(slot.displayName)")
                                    if let title = slot.title {
                                        Text(title).font(.caption).foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                    }
                }

                Section("Install Controls") {
                    Text("Open Control Center, touch and hold an empty area, tap Add a Control, then add Omni Live Stream up to four times. Configure each copy as Live Slot 1, 2, 3, or 4.")
                    Text("Open this app after changing controls so their Apple push tokens are synced immediately.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }

                Section("Diagnostics") {
                    LabeledContent("Device ID", value: OmniSettings.shared.deviceId)
                    LabeledContent("Push Environment", value: OmniSettings.shared.apnsEnvironment)
                    LabeledContent(
                        "Registered Controls",
                        value: String(OmniSettings.shared.lastRegistrationCount)
                    )
                    if let date = OmniSettings.shared.lastRegistrationAt {
                        LabeledContent("Last Registration", value: date.formatted())
                    }
                    if OmniSettings.shared.pendingRegistrations != nil {
                        LabeledContent("Registration Sync", value: "Pending Retry")
                    }
                    if let error = OmniSettings.shared.lastRegistrationError {
                        Text("Last registration error: \(error)")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                    if let diagnostics = model.diagnostics {
                        LabeledContent(
                            "Server APNs",
                            value: diagnostics.apnsEnabled ? "Enabled" : "Disabled"
                        )
                        LabeledContent(
                            "Server Registrations",
                            value: String(diagnostics.registrationCount)
                        )
                        LabeledContent(
                            "Undelivered Controls",
                            value: String(diagnostics.undeliveredCount)
                        )
                        if let timestamp = diagnostics.lastReconciledAt {
                            LabeledContent(
                                "Last Live Reconcile",
                                value: Date(timeIntervalSince1970: timestamp / 1_000).formatted()
                            )
                        }
                    }
                }
            }
            .navigationTitle("Omni Live")
            .task { await model.refreshIfConfigured() }
            .onChange(of: scenePhase) { _, phase in
                guard phase == .active else { return }
                Task { await model.refreshIfConfigured() }
            }
        }
    }
}
