import Foundation
import WidgetKit

enum ControlRegistrationSync {
    static func registrations(from controls: [ControlInfo]) -> [ControlRegistration] {
        let environment = OmniSettings.shared.apnsEnvironment
        return controls.compactMap { control in
            guard control.kind == OmniControlDefinition.kind,
                  let token = control.pushInfo?.token,
                  let intent = control.configurationIntent(of: LiveSlotConfigurationIntent.self)
            else { return nil }
            return ControlRegistration(
                controlId: control.id,
                slot: intent.slot.rawValue,
                pushToken: token.hexString,
                environment: environment
            )
        }
    }

    static func syncCurrentControls() async throws -> Int {
        let controls = try await ControlCenter.shared.currentControls()
        let registrations = registrations(from: controls)
        OmniSettings.shared.pendingRegistrations = registrations
        try await RegistrationRetry.shared.flush(maxAttempts: 3)
        return registrations.count
    }

    static func queue(_ registrations: [ControlRegistration]) {
        OmniSettings.shared.pendingRegistrations = registrations
        Task {
            do {
                try await RegistrationRetry.shared.flush(maxAttempts: 5)
            } catch {
                OmniSettings.shared.lastRegistrationError = error.localizedDescription
            }
        }
    }
}

private actor RegistrationRetry {
    static let shared = RegistrationRetry()

    func flush(maxAttempts: Int) async throws {
        var finalError: Error?
        for attempt in 0 ..< maxAttempts {
            guard let registrations = OmniSettings.shared.pendingRegistrations else { return }
            do {
                try await OmniAPIClient().register(controls: registrations)
                if OmniSettings.shared.pendingRegistrations == registrations {
                    OmniSettings.shared.pendingRegistrations = nil
                    OmniSettings.shared.lastRegistrationError = nil
                }
                return
            } catch {
                finalError = error
                OmniSettings.shared.lastRegistrationError = error.localizedDescription
                guard attempt + 1 < maxAttempts else { break }
                try? await Task.sleep(for: .seconds(1 << attempt))
            }
        }
        throw finalError ?? OmniAPIError.invalidResponse
    }
}

private extension Data {
    var hexString: String { map { String(format: "%02x", $0) }.joined() }
}
