import Foundation

enum LiveSlotResolution {
    static func currentState(
        fetch: () async throws -> LiveSlotState,
        cached: () -> LiveSlotState?
    ) async throws -> LiveSlotState {
        do {
            return try await fetch()
        } catch {
            if let cached = cached() { return cached }
            throw error
        }
    }

    static func tapURL(
        fetch: () async throws -> LiveSlotState,
        cached: () -> LiveSlotState?,
        homeURL: URL
    ) async -> URL {
        do {
            return try await fetch().url
        } catch {
            return cached()?.url ?? homeURL
        }
    }
}
