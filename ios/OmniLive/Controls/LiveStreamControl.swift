import AppIntents
import SwiftUI
import WidgetKit

struct LiveSlotValueProvider: AppIntentControlValueProvider {
    func previewValue(configuration: LiveSlotConfigurationIntent) -> LiveSlotState {
        .preview(slot: configuration.slot.rawValue)
    }

    func currentValue(configuration: LiveSlotConfigurationIntent) async throws -> LiveSlotState {
        let slot = configuration.slot.rawValue
        return try await LiveSlotResolution.currentState(
            fetch: { try await OmniAPIClient().liveSlot(slot) },
            cached: { OmniSettings.shared.cachedSlot(slot) }
        )
    }
}

struct OpenLiveSlotIntent: AppIntent {
    static let title: LocalizedStringResource = "Open Omni Live Stream"

    @Parameter(title: "Slot")
    var slot: Int

    init() {
        slot = 1
    }

    init(slot: Int) {
        self.slot = slot
    }

    func perform() async throws -> some IntentResult & OpensIntent {
        let home = OmniSettings.shared.serverURL ?? URL(string: "http://omni.boris")!
        let url = await LiveSlotResolution.tapURL(
            fetch: { try await OmniAPIClient().liveSlot(slot) },
            cached: { OmniSettings.shared.cachedSlot(slot) },
            homeURL: home
        )
        return .result(opensIntent: OpenURLIntent(url))
    }
}

struct LiveStreamControl: ControlWidget {
    var body: some ControlWidgetConfiguration {
        AppIntentControlConfiguration(
            kind: OmniControlDefinition.kind,
            provider: LiveSlotValueProvider()
        ) { value in
            ControlWidgetButton(action: OpenLiveSlotIntent(slot: value.slot)) {
                Label(
                    value.displayName,
                    systemImage: value.isLive ? "play.fill" : "moon.zzz.fill"
                )
                .controlWidgetStatus(value.title ?? "Open Omni")
            }
            .tint(value.isLive ? .red : .gray)
        }
        .displayName("Omni Live Stream")
        .description("Opens a currently live channel from Omni.")
        .pushHandler(OmniControlPushHandler.self)
    }
}

struct OmniControlPushHandler: ControlPushHandler {
    init() {}

    func pushTokensDidChange(controls: [ControlInfo]) {
        let registrations = ControlRegistrationSync.registrations(from: controls)
        ControlRegistrationSync.queue(registrations)
    }
}
