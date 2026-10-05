import Foundation
import HermesKit
import Security

/// Agent secrets in the Keychain, this device only, readable after first unlock
/// (so the notification extension can use them while the phone is locked).
enum Keychain {
    private static let service = "io.github.bipagents.agent"

    static func save(_ secrets: AgentSecrets, for agentID: UUID) throws {
        let payload = try JSONEncoder().encode(StoredSecrets(apiKey: secrets.apiKey, bridgeKey: secrets.bridgeKey))
        let query = baseQuery(agentID)
        SecItemDelete(query as CFDictionary)
        var item = query
        item[kSecValueData as String] = payload
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else { throw KeychainError(status: status) }
    }

    static func secrets(for agentID: UUID) -> AgentSecrets? {
        var query = baseQuery(agentID)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data,
              let stored = try? JSONDecoder().decode(StoredSecrets.self, from: data) else { return nil }
        return AgentSecrets(apiKey: stored.apiKey, bridgeKey: stored.bridgeKey)
    }

    static func delete(for agentID: UUID) {
        SecItemDelete(baseQuery(agentID) as CFDictionary)
    }

    private static func baseQuery(_ agentID: UUID) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: agentID.uuidString]
    }

    private struct StoredSecrets: Codable {
        var apiKey: String
        var bridgeKey: String?
    }
}

struct KeychainError: Error, LocalizedError {
    var status: OSStatus
    var errorDescription: String? {
        SecCopyErrorMessageString(status, nil) as String? ?? String(localized: "Erreur Trousseau \(status)")
    }
}
