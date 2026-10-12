import AppIntents

enum OmniControlDefinition {
    static let kind = "com.micthiesen.OmniLive.live-stream"
}

enum LiveSlotChoice: Int, AppEnum {
    case one = 1
    case two = 2
    case three = 3
    case four = 4

    static let typeDisplayRepresentation = TypeDisplayRepresentation(name: "Live Slot")
    static let caseDisplayRepresentations: [LiveSlotChoice: DisplayRepresentation] = [
        .one: "Live Slot 1",
        .two: "Live Slot 2",
        .three: "Live Slot 3",
        .four: "Live Slot 4",
    ]
}

struct LiveSlotConfigurationIntent: ControlConfigurationIntent {
    static let title: LocalizedStringResource = "Omni Live Slot"
    static let description = IntentDescription("Choose which ranked live channel this control opens.")

    @Parameter(title: "Slot", default: .one)
    var slot: LiveSlotChoice
}
