import HermesKit
import VoiceKit

extension AgentStore {
    /// Kyutai through the agent's bridge when configured, the iPhone's own voice otherwise (and as fallback).
    func ttsProvider(for agent: AgentProfile) -> any TTSProvider {
        guard let bridgeURL = agent.config.bridgeURL, let key = secrets(for: agent)?.bridgeKey else {
            #if DEBUG
            print("[voice] \(agent.name): no bridge configured (url: \(agent.config.bridgeURL != nil), key: \(secrets(for: agent)?.bridgeKey != nil)) → system voice")
            #endif
            return SystemTTSProvider()
        }
        return FallbackTTSProvider(primary: BridgeTTSProvider(bridgeURL: bridgeURL, bridgeKey: key, voice: agent.voice),
                                   fallback: SystemTTSProvider())
    }
}
