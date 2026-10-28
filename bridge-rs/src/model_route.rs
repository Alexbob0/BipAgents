//! Provider routing at EUrouter (2026-10-26): every model call names the providers to try first, so the provider's
//! prompt cache works. Unpinned, EUrouter picks one of seven EU providers per call: the agent's instructions and tools
//! (≈ 17k tokens) were never read from a cache and calls cost ≈ $0.20/M input. Pinned (Entrim), the next calls with the
//! same prefix read ≈ 99 % of it from the cache at 17.5 % of the input price. `allow_fallbacks`: when the providers
//! named are down, EUrouter still picks another one (availability wins).
//!
//! The order is the bridge's setting `[model] provider_order` (default [`DEFAULT_ORDER`]; `[]` turns pinning off).
//! It reaches:
//! - Hermes, through each agent profile's `providers:` entry for the router's address (Hermes 517b5e10f merges a
//!   custom endpoint's `extra_body` into every request to it, the auxiliary ones included: no patch needed). Hermes
//!   uses the FIRST entry whose address matches, so the thinking entry (agents_admin.rs, `THINKING_PROVIDER`) carries
//!   the routing too and the routing entry is then left out. Written at each start for every agent (`agent_tools::
//!   register`), at a claim, and when the person changes the model or the thinking level;
//! - the bridge's own calls (Carnet, rooms' router, records, mail scan, WhatsApp import): [`add_to_body`].

use serde_json::{json, Value};

use crate::agents_admin::pyyaml::Yaml;
use crate::config::{AgentConfig, Config};

/// Entrim first, Melious second (measured 2026-10-26, see the module's doc).
pub const DEFAULT_ORDER: [&str; 2] = ["entrim", "melious"];

/// The `providers:` entry holding the routing when the thinking entry doesn't.
pub const ROUTE_PROVIDER: &str = "bipagents-route";

/// Whether calls to `base_url` are routed: EUrouter only (another router or server ignores or refuses the field).
pub fn pins(base_url: &str) -> bool {
    url::Url::parse(base_url.trim())
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .is_some_and(|host| host.split('.').any(|part| part == "eurouter"))
}

/// The request's `provider` object for `base_url`, or None (not EUrouter, or no order set).
pub fn routing(order: &[String], base_url: &str) -> Option<Value> {
    (!order.is_empty() && pins(base_url)).then(|| json!({"order": order, "allow_fallbacks": true}))
}

/// `body["provider"]` set for a call to `base_url` (left alone when it isn't routed), and GLM's thinking floor
/// ([`add_thinking_floor`]).
pub fn add_to_body(order: &[String], base_url: &str, body: &mut Value) {
    add_thinking_floor(base_url, body);
    if let (Some(route), Some(map)) = (routing(order, base_url), body.as_object_mut()) {
        map.insert("provider".into(), route);
    }
}

/// The bridge's own calls to GLM through EUrouter (Carnet, rooms' router, records, mail scan, memory language) ask
/// for « low » thinking (agents_admin::keeps_thinking_on): sent without, GLM thinks at its own default and a call with
/// a few output tokens (the rooms' router: 20) can end before any answer. Left alone when the body sets a level.
pub fn add_thinking_floor(base_url: &str, body: &mut Value) {
    let model = body.get("model").and_then(Value::as_str).unwrap_or("");
    if !crate::agents_admin::keeps_thinking_on(model, base_url) {
        return;
    }
    if let Some(map) = body.as_object_mut() {
        map.entry("reasoning_effort").or_insert_with(|| json!(crate::agents_admin::GLM_THINKING));
    }
}

/// A Hermes config with the routing for its model's address (`model.provider: custom`), or None when nothing changes.
/// GLM through EUrouter also gets its thinking floor there (agents_admin::with_thinking_floor): every path that
/// writes a profile's model passes here.
pub fn with_routing(data: &Yaml, order: &[String]) -> Option<Yaml> {
    let floored = crate::agents_admin::with_thinking_floor(data);
    let data = floored.as_ref().unwrap_or(data);
    let routed = with_routing_only(data, order);
    routed.or(floored)
}

fn with_routing_only(data: &Yaml, order: &[String]) -> Option<Yaml> {
    let model = data.get("model").filter(|m| m.is_map());
    let custom = model.and_then(|m| m.get("provider")).and_then(Yaml::as_str) == Some("custom");
    let base = model.and_then(|m| m.get("base_url")).and_then(Yaml::as_str).unwrap_or("").to_string();
    let route = if custom { routing(order, &base) } else { None };
    let mut out = data.clone();
    let mut providers = out.get("providers").filter(|p| p.is_map()).cloned().unwrap_or_else(Yaml::map);
    let thinking = providers.get(crate::agents_admin::THINKING_PROVIDER).is_some_and(Yaml::is_map);
    if thinking {
        if let Some(entry) = providers.get_mut(crate::agents_admin::THINKING_PROVIDER) {
            let extra = entry.entry_map("extra_body");
            match (&route, extra) {
                (Some(route), Some(extra)) if extra.is_map() => extra.insert("provider", crate::agents_admin::pyyaml::from_json(route)),
                (None, Some(extra)) => {
                    extra.remove("provider");
                }
                _ => {}
            }
        }
    }
    match &route {
        Some(route) if !thinking => {
            let mut extra = Yaml::map();
            extra.insert("provider", crate::agents_admin::pyyaml::from_json(route));
            let mut entry = Yaml::map();
            entry.insert("api", Yaml::str(&base));
            entry.insert("extra_body", extra);
            providers.insert(ROUTE_PROVIDER, entry);
        }
        _ => {
            providers.remove(ROUTE_PROVIDER);
        }
    }
    if providers.truthy() {
        out.insert("providers", providers);
    } else {
        out.remove("providers");
    }
    (out != *data).then_some(out)
}

/// The routing written in a Hermes config file; whether it changed.
pub fn write(path: &std::path::Path, order: &[String]) -> anyhow::Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let data = crate::agents_admin::load_or_empty(path)?;
    match with_routing(&data, order) {
        Some(changed) => {
            crate::agents_admin::replace_yaml(path, &changed)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// The agent's profile routed as the bridge's setting says (Hermes reads it again from the next conversation).
pub fn apply(config: &Config, agent: &AgentConfig) -> anyhow::Result<bool> {
    let Some(folder) = crate::shares::profile_dir(config, agent) else { return Ok(false) };
    let changed = write(&folder.join("config.yaml"), &config.model_provider_order)?;
    if changed {
        tracing::info!(target: "bipbridge::agents", agent = %agent.name, "model provider routing set");
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents_admin::pyyaml;

    fn order() -> Vec<String> {
        DEFAULT_ORDER.iter().map(|s| s.to_string()).collect()
    }

    const EU: &str = "model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n";

    #[test]
    fn only_eurouter_is_routed() {
        assert!(pins("https://api.eurouter.ai/api/v1"));
        assert!(pins("https://api.eurouter.example/v1"));
        assert!(!pins("https://openrouter.ai/api/v1"));
        assert!(!pins("http://gpu:8888/v1"));
        assert!(!pins("not a url"));
        assert_eq!(routing(&[], "https://api.eurouter.ai/api/v1"), None);
        let mut body = json!({"model": "m"});
        add_to_body(&order(), "https://api.eurouter.ai/api/v1", &mut body);
        assert_eq!(body["provider"], json!({"order": ["entrim", "melious"], "allow_fallbacks": true}));
        let mut other = json!({"model": "m"});
        add_to_body(&order(), "http://gpu:8888/v1", &mut other);
        assert!(other.get("provider").is_none());
    }

    /// GLM through EUrouter thinks at « low » (agents_admin::keeps_thinking_on).
    const LOW: &str = "agent:\n  reasoning_effort: low\n";

    #[test]
    fn a_profile_gets_one_entry_for_the_router_and_keeps_it() {
        let data = pyyaml::load(EU).unwrap();
        let routed = with_routing(&data, &order()).unwrap();
        assert_eq!(
            pyyaml::dump(&routed),
            format!(
                "{EU}{LOW}providers:\n  bipagents-route:\n    api: https://api.eurouter.ai/api/v1\n    extra_body:\n      provider:\n        order:\n        - entrim\n        - melious\n        allow_fallbacks: true\n"
            )
        );
        assert_eq!(with_routing(&routed, &order()), None); // nothing more to do
        // turned off: the entry goes, and `providers` with it; GLM's floor stays
        assert_eq!(pyyaml::dump(&with_routing(&routed, &[]).unwrap()), format!("{EU}{LOW}"));
    }

    #[test]
    fn glm_through_the_router_never_goes_below_low() {
        // nothing chosen, or « none » (GLM answers nothing to it there): « low »; pinning off changes nothing to that
        for effort in ["", "agent:\n  reasoning_effort: none\n", "agent:\n  reasoning_effort: false\n"] {
            let data = pyyaml::load(&format!("{EU}{effort}")).unwrap();
            assert_eq!(pyyaml::dump(&with_routing(&data, &[]).unwrap()), format!("{EU}{LOW}"), "{effort}");
        }
        // a level above, another model or another server: untouched
        for text in [
            format!("{EU}agent:\n  reasoning_effort: medium\n"),
            EU.replace("glm-5.3-flash", "deepseek/deepseek-v4.1-flash"),
            EU.replace("https://api.eurouter.ai/api/v1", "http://sparks:8000/v1"),
            format!("{}agent:\n  reasoning_effort: none\n", EU.replace("https://api.eurouter.ai/api/v1", "http://sparks:8000/v1")),
        ] {
            assert_eq!(with_routing(&pyyaml::load(&text).unwrap(), &[]), None, "{text}");
        }
    }

    #[test]
    fn the_bridges_own_calls_to_glm_ask_for_low() {
        let mut body = json!({"model": "glm-5.3-flash", "max_tokens": 20});
        add_to_body(&order(), "https://api.eurouter.ai/api/v1", &mut body);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["provider"]["allow_fallbacks"], true); // Melious, which refuses the field, is skipped
        let mut set = json!({"model": "GLM-5.3", "reasoning_effort": "high"});
        add_thinking_floor("https://api.eurouter.ai/api/v1", &mut set);
        assert_eq!(set["reasoning_effort"], "high");
        for (model, base) in [
            ("deepseek/deepseek-v4.1-flash", "https://api.eurouter.ai/api/v1"),
            ("glm-5.3", "http://sparks:8000/v1"),
            ("z-ai/glm-5.3", "https://openrouter.ai/api/v1"),
        ] {
            let mut other = json!({"model": model});
            add_to_body(&order(), base, &mut other);
            assert!(other.get("reasoning_effort").is_none(), "{model} {base}");
        }
    }

    #[test]
    fn the_thinking_entry_carries_the_routing() {
        let text = format!(
            "{EU}providers:\n  bipagents-route:\n    api: https://api.eurouter.ai/api/v1\n  bipagents-thinking:\n    api: https://api.eurouter.ai/api/v1\n    extra_body:\n      chat_template_kwargs:\n        enable_thinking: false\n"
        );
        let routed = with_routing(&pyyaml::load(&text).unwrap(), &order()).unwrap();
        let providers = routed.get("providers").unwrap();
        assert!(providers.get(ROUTE_PROVIDER).is_none());
        let extra = providers.get("bipagents-thinking").and_then(|e| e.get("extra_body")).unwrap();
        assert!(extra.get("chat_template_kwargs").is_some());
        assert_eq!(extra.get("provider").and_then(|p| p.get("allow_fallbacks")), Some(&Yaml::Bool(true)));
    }

    #[test]
    fn other_models_are_left_alone() {
        for text in [
            "model:\n  provider: custom\n  base_url: http://gpu:8888/v1\n",
            "model:\n  provider: openrouter\n  default: x\n",
            "agent:\n  max_turns: 60\n",
        ] {
            assert_eq!(with_routing(&pyyaml::load(text).unwrap(), &order()), None, "{text}");
        }
    }
}
