import Intents
import UserNotifications
import WidgetKit

/// Turns streamer alerts into communication notifications (the streamer's face as the
/// sender avatar), picks the action category, and reloads widgets the push names.
///
/// Every failure path delivers the original content unchanged. The avatar download is
/// bounded (8 s, 2 MB, decoded straight to a 256 px thumbnail) to stay well inside the
/// extension's ~30 s and ~24 MB limits.
final class NotificationService: UNNotificationServiceExtension {
    private let state = DeliveryState()

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        state.begin(original: request.content, handler: ContentHandler(call: contentHandler))
        let prepared = Self.prepare(request.content)
        let tracker = state
        Self.start(prepared, state: tracker)
    }

    private static func start(_ prepared: Prepared, state: DeliveryState) {
        Task { await deliver(prepared, state: state) }
    }

    /// Category and widget reloads need no network; only the avatar does.
    private static func prepare(_ original: UNNotificationContent) -> Prepared {
        let payload = OmniPushPayload.parse(userInfo: original.userInfo)
        for kind in payload?.widgetKindsToReload ?? [] {
            WidgetCenter.shared.reloadTimelines(ofKind: kind)
        }
        let content = (original.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
        content.categoryIdentifier = OmniNotificationCategory.category(
            for: payload, current: original.categoryIdentifier
        )
        return Prepared(content: content, sender: payload?.sender)
    }

    private static func deliver(_ prepared: Prepared, state: DeliveryState) async {
        guard let sender = prepared.sender else {
            state.finish(prepared.content)
            return
        }
        let avatar = await avatarData(sender.avatarURL)
        let updated = await CommunicationContent.make(from: prepared.content, sender: sender, avatarPNG: avatar)
        state.finish(updated ?? prepared.content)
    }

    override func serviceExtensionTimeWillExpire() {
        state.finishWithOriginal()
    }

    private static func avatarData(_ url: URL?) async -> Data? {
        guard let url else { return nil }
        // AvatarStore caches the thumbnail in the App Group, shared with widgets.
        guard let avatar = await AvatarStore.avatar(for: url, maxPixels: 256, timeout: 8) else {
            return nil
        }
        return AvatarStore.pngData(avatar.image)
    }
}

/// `UNMutableNotificationContent` is not `Sendable`; it is only touched by one task.
private struct Prepared: @unchecked Sendable {
    let content: UNMutableNotificationContent
    let sender: OmniPushPayload.Sender?
}

private struct ContentHandler: @unchecked Sendable {
    let call: (UNNotificationContent) -> Void
}

/// Calls the content handler exactly once, whichever of completion and expiry wins.
private final class DeliveryState: @unchecked Sendable {
    private let lock = NSLock()
    private var handler: ContentHandler?
    private var original: UNNotificationContent?

    func begin(original: UNNotificationContent, handler: ContentHandler) {
        lock.withLock {
            self.original = original
            self.handler = handler
        }
    }

    func finish(_ content: UNNotificationContent) {
        let handler = lock.withLock { () -> ContentHandler? in
            defer { self.handler = nil }
            return self.handler
        }
        handler?.call(content)
    }

    func finishWithOriginal() {
        let pair = lock.withLock { () -> (ContentHandler?, UNNotificationContent?) in
            defer { self.handler = nil }
            return (self.handler, self.original)
        }
        if let handler = pair.0, let original = pair.1 { handler.call(original) }
    }
}

enum CommunicationContent {
    /// Builds an incoming `INSendMessageIntent` from the sender, donates it, and returns
    /// the content updated from it, or nil if iOS rejects the update.
    static func make(
        from content: UNMutableNotificationContent,
        sender: OmniPushPayload.Sender,
        avatarPNG: Data?
    ) async -> UNNotificationContent? {
        let lines = CommunicationText.lines(
            title: content.title, subtitle: content.subtitle, body: content.body, senderName: sender.name
        )
        content.subtitle = lines.subtitle
        content.body = lines.body
        let image = avatarPNG.map { INImage(imageData: $0) }
        let person = INPerson(
            personHandle: INPersonHandle(value: sender.id, type: .unknown),
            nameComponents: nil,
            displayName: sender.name,
            image: image,
            contactIdentifier: nil,
            customIdentifier: "omni-streamer:\(sender.id)"
        )
        let intent = INSendMessageIntent(
            recipients: nil,
            outgoingMessageType: .outgoingMessageText,
            content: content.body,
            speakableGroupName: nil,
            conversationIdentifier: sender.id,
            serviceName: nil,
            sender: person,
            attachments: nil
        )
        if let image {
            intent.setImage(image, forParameterNamed: \.sender)
        }
        let interaction = INInteraction(intent: intent, response: nil)
        interaction.direction = .incoming
        // A failed donation still lets the content update; it only affects suggestions.
        try? await interaction.donate()
        guard let updated = try? content.updating(from: intent) else { return nil }
        // Keep the server's context lines if the update dropped them.
        if updated.subtitle != lines.subtitle || updated.body != lines.body,
           let copy = updated.mutableCopy() as? UNMutableNotificationContent
        {
            copy.subtitle = lines.subtitle
            copy.body = lines.body
            return copy
        }
        return updated
    }
}
