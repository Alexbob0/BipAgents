import CryptoKit
import Foundation
import HermesKit
import Security
import Synchronization

/// The local-network way to an agent, for when Tailscale is down (no Internet at home, the VPN off): the bridge's
/// LAN door, over HTTPS with its own self-signed certificate, accepted only if its SHA-256 matches the one from the
/// pairing QR code. Hermes is reached through the bridge there (`/hermes/<agent>/…`). The tailnet stays the normal
/// way: `AgentStore.refreshReachability` switches an agent to the door only when the tailnet does not answer.
nonisolated enum LocalLink {
    private struct State: Sendable {
        var active: Set<UUID> = []
        var pins: [String: String] = [:] // "host:port" -> certificate SHA-256 (hex)
    }

    private static let state = Mutex(State())

    /// Requests to LAN doors: the certificate is checked against the pinned fingerprint, nothing else.
    static let session = URLSession(configuration: .default, delegate: PinningDelegate(), delegateQueue: nil)

    static func isActive(_ agent: AgentProfile) -> Bool {
        state.withLock { $0.active.contains(agent.id) }
    }

    static func setActive(_ active: Bool, for agent: AgentProfile) {
        state.withLock { state in
            if active, agent.config.lanURL != nil { state.active.insert(agent.id) } else { state.active.remove(agent.id) }
        }
    }

    /// The agents' LAN certificates, refreshed whenever the agents change.
    static func updatePins(for agents: [AgentProfile]) {
        var pins: [String: String] = [:]
        for agent in agents {
            guard let url = agent.config.lanURL, let host = url.host(), let fingerprint = agent.config.lanFingerprint else { continue }
            pins["\(host):\(url.port ?? 443)"] = fingerprint
        }
        state.withLock { state in
            state.pins = pins
            state.active = state.active.filter { id in agents.contains { $0.id == id && $0.config.lanURL != nil } }
        }
    }

    /// Hermes' address right now: on the tailnet, or through the LAN door.
    static func hermesURL(for agent: AgentProfile) -> URL {
        guard isActive(agent), let door = agent.config.lanURL else { return agent.config.baseURL }
        return door.appending(path: "hermes/\(agent.bridgeName)")
    }

    /// The bridge's address right now (nil without a bridge).
    static func bridgeURL(for agent: AgentProfile) -> URL? {
        guard isActive(agent), let door = agent.config.lanURL else { return agent.config.bridgeURL }
        return door
    }

    static func session(for agent: AgentProfile) -> URLSession {
        isActive(agent) ? session : .shared
    }

    fileprivate static func pin(host: String, port: Int) -> String? {
        state.withLock { $0.pins["\(host):\(port)"] }
    }

    /// SHA-256 (hex) of a certificate's DER bytes.
    static func fingerprint(of certificate: SecCertificate) -> String {
        SHA256.hash(data: SecCertificateCopyData(certificate) as Data).map { String(format: "%02x", $0) }.joined()
    }
}

/// Accepts a LAN door's certificate when it is the pinned one; every other server goes through the usual checks.
private nonisolated final class PinningDelegate: NSObject, URLSessionDelegate, Sendable {
    func urlSession(_ session: URLSession, didReceive challenge: URLAuthenticationChallenge) async
        -> (URLSession.AuthChallengeDisposition, URLCredential?) {
        let space = challenge.protectionSpace
        guard space.authenticationMethod == NSURLAuthenticationMethodServerTrust, let trust = space.serverTrust,
              let expected = LocalLink.pin(host: space.host, port: space.port) else {
            return (.performDefaultHandling, nil)
        }
        guard let leaf = (SecTrustCopyCertificateChain(trust) as? [SecCertificate])?.first,
              LocalLink.fingerprint(of: leaf) == expected else {
            return (.cancelAuthenticationChallenge, nil)  // not our bridge: refuse, whatever it says
        }
        return (.useCredential, URLCredential(trust: trust))
    }
}

/// Runs `operation`, giving up after `seconds` (a tailnet that does not answer can otherwise hang for a minute).
nonisolated func withTimeout<T: Sendable>(_ seconds: Double, _ operation: @escaping @Sendable () async throws -> T) async throws -> T {
    try await withThrowingTaskGroup(of: T.self) { group in
        group.addTask { try await operation() }
        group.addTask {
            try await Task.sleep(for: .seconds(seconds))
            throw URLError(.timedOut)
        }
        defer { group.cancelAll() }
        return try await group.next()!
    }
}
