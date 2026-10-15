import Foundation
import HermesKit
import VoiceKit

extension AgentStore {
    /// The agent's voice for `use` through its bridge when configured, the iPhone's own voice in the agent's language
    /// otherwise (and as fallback: the bridge fails rather than read Spanish with a French voice).
    func ttsProvider(for agent: AgentProfile, use: VoiceUse = .live) -> any TTSProvider {
        let system = SystemTTSProvider(language: agent.language.locale.identifier(.bcp47))
        if agent.usesSystemVoice(for: use) { return system }
        guard let bridgeURL = LocalLink.bridgeURL(for: agent), let key = secrets(for: agent)?.bridgeKey else {
            #if DEBUG
            print("[voice] \(agent.name): no bridge configured (url: \(agent.config.bridgeURL != nil), key: \(secrets(for: agent)?.bridgeKey != nil)) → system voice")
            #endif
            return system
        }
        return FallbackTTSProvider(primary: BridgeTTSProvider(bridgeURL: bridgeURL, bridgeKey: key, voice: agent.voice(for: use),
                                                              session: LocalLink.session(for: agent)),
                                   fallback: system)
    }
}
