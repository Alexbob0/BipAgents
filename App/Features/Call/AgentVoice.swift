import Foundation
import HermesKit
import VoiceKit

extension AgentStore {
    /// Kyutai through the agent's bridge when configured, the iPhone's own voice otherwise (and as fallback,
    /// and for languages the bridge voices don't speak yet).
    func ttsProvider(for agent: AgentProfile) -> any TTSProvider {
        let system = SystemTTSProvider(language: agent.language.locale.identifier(.bcp47))
        guard agent.language.usesBridgeVoice else { return system }
        guard let bridgeURL = agent.config.bridgeURL, let key = secrets(for: agent)?.bridgeKey else {
            #if DEBUG
            print("[voice] \(agent.name): no bridge configured (url: \(agent.config.bridgeURL != nil), key: \(secrets(for: agent)?.bridgeKey != nil)) → system voice")
            #endif
            return system
        }
        return FallbackTTSProvider(primary: BridgeTTSProvider(bridgeURL: bridgeURL, bridgeKey: key, voice: agent.voice),
                                   fallback: system)
    }
}
