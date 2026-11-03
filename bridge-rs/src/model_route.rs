//! Provider routing at EUrouter (2026-10-26): every model call names the providers to try first, so the provider's
//! prompt cache works. Unpinned, EUrouter picks one of seven EU providers per call: the agent's instructions and tools
//! (≈ 17k tokens) were never read from a cache and calls cost ≈ $0.20/M input. Pinned (Entrim), the next calls with the
//! same prefix read ≈ 99 % of it from the cache at 17.5 % of the input price. A person must stay with ONE provider:
//! the cache is the provider's, so every call served elsewhere pays its prefix in full (up to 2.5× the price).
//!
//! The order, GLM-5.3 Flash per provider (2026-10-31, `allow_fallbacks: false`, two calls on a 9.7k-token prefix,
//! reasoning « low »; uncached input, then the cached part of the second call):
//! - Entrim: $0.10/M, cache 9 664/9 702 billed ≈ 17.5 % of that ($0.0175/M), the best;
//! - GreenPT: ≈ €0.11/M, cache 9 472 at ≈ 20 % (€0.022/M), the second;
//! - Inceptron: $0.15/M, cache 8 960 at ≈ 47 % ($0.07/M), the third;
//! - left out: Lyceum ($0.20/M, partial cache), Tensorix ($0.20/M, no cache), Berget (€0.25/M, reports a cache but
//!   bills it in full), and Melious, which refuses GLM with a reasoning field (HTTP 400 « does not support
//!   reasoning ») when the bridge always sends one (agents_admin::keeps_thinking_on): it was second until then, so
//!   when Entrim failed the calls spread over any provider, with no cache.
//!
//! `allow_fallbacks`: when the three named are all down, EUrouter still picks another one (availability wins).
//!
//! The order is the bridge's setting `[model] provider_order` (default [`DEFAULT_ORDER`]; `[]` turns pinning off).
//! It reaches:
//! - Hermes, through each agent profile's `providers:` entry for the router's address (Hermes 517b5e10f merges a
//!   custom endpoint's `extra_body` into every request to it, the auxiliary ones included: no patch needed). Hermes
//!   uses the FIRST entry whose address matches, so the thinking entry (agents_admin.rs, `THINKING_PROVIDER`) carries
//!   the routing too and the routing entry is then left out. Written at each start for every agent (`agent_tools::
//!   register`, which also routes an agent just cloned), at a claim, and when the person changes the model or the
//!   thinking level. The hub's own config.yaml (agents are cloned from it) only at a claim or a new AI key;
//! - the bridge's own calls (Carnet, rooms' router, records, mail scan, WhatsApp import): [`add_to_body`].

use serde_json::{json, Value};

use crate::agents_admin::pyyaml::Yaml;

/// Entrim, then GreenPT, then Inceptron: the three that cache GLM best (measured 2026-10-31, see the module's doc).
pub const DEFAULT_ORDER: [&str; 3] = ["entrim", "greenpt", "inceptron"];

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
/// writes a profile's model passes here. `chosen`: the level the person chose for that profile
/// (agents_admin::chosen_thinking), None when they never did.
pub fn with_routing(data: &Yaml, order: &[String], chosen: Option<&str>) -> Option<Yaml> {
    let floored = crate::agents_admin::with_thinking_floor(data, chosen);
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

/// The routing written in a Hermes config file (the hub's, new agents are cloned from); whether it changed.
pub fn write(path: &std::path::Path, order: &[String]) -> anyhow::Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let chosen = path.parent().and_then(crate::agents_admin::chosen_thinking);
    crate::profile_files::edit(path, |data| {
        if let Some(routed) = with_routing(data, order, chosen.as_deref()) {
            *data = routed;
        }
    })
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
        assert_eq!(body["provider"], json!({"order": ["entrim", "greenpt", "inceptron"], "allow_fallbacks": true}));
        let mut other = json!({"model": "m"});
        add_to_body(&order(), "http://gpu:8888/v1", &mut other);
        assert!(other.get("provider").is_none());
    }

    /// GLM through EUrouter thinks at « low » (agents_admin::keeps_thinking_on).
    const LOW: &str = "agent:\n  reasoning_effort: low\n";

    #[test]
    fn a_profile_gets_one_entry_for_the_router_and_keeps_it() {
        let data = pyyaml::load(EU).unwrap();
        let routed = with_routing(&data, &order(), None).unwrap();
        assert_eq!(
            pyyaml::dump(&routed),
            format!(
                "{EU}{LOW}providers:\n  bipagents-route:\n    api: https://api.eurouter.ai/api/v1\n    extra_body:\n      provider:\n        order:\n        - entrim\n        - greenpt\n        - inceptron\n        allow_fallbacks: true\n"
            )
        );
        assert_eq!(with_routing(&routed, &order(), None), None); // nothing more to do
        // a profile routed with the old order (entrim, melious, before 2026-10-31) gets the new one at the next start
        let old = pyyaml::dump(&routed).replace("- greenpt\n        - inceptron\n", "- melious\n");
        let again = with_routing(&pyyaml::load(&old).unwrap(), &order(), None).unwrap();
        assert_eq!(pyyaml::dump(&again), pyyaml::dump(&routed));
        // turned off: the entry goes, and `providers` with it; GLM's floor stays
        assert_eq!(pyyaml::dump(&with_routing(&routed, &[], None).unwrap()), format!("{EU}{LOW}"));
    }

    #[test]
    fn glm_through_the_router_never_goes_below_low() {
        // nothing chosen, or « none » (GLM answers nothing to it there): « low »; pinning off changes nothing to that
        for effort in ["", "agent:\n  reasoning_effort: none\n", "agent:\n  reasoning_effort: false\n"] {
            let data = pyyaml::load(&format!("{EU}{effort}")).unwrap();
            assert_eq!(pyyaml::dump(&with_routing(&data, &[], None).unwrap()), format!("{EU}{LOW}"), "{effort}");
        }
        // Hermes' template « medium » is no choice (real-day bench 2026-10-28): « low » too, unless the person chose it
        let medium = pyyaml::load(&format!("{EU}agent:\n  reasoning_effort: medium\n")).unwrap();
        assert_eq!(pyyaml::dump(&with_routing(&medium, &[], None).unwrap()), format!("{EU}{LOW}"));
        assert_eq!(with_routing(&medium, &[], Some("medium")), None);
        // a chosen level a rewrite lost comes back; « none » chosen is written « low »
        let low = pyyaml::load(&format!("{EU}{LOW}")).unwrap();
        let back = with_routing(&low, &[], Some("high")).unwrap();
        assert_eq!(pyyaml::dump(&back), format!("{EU}agent:\n  reasoning_effort: high\n"));
        assert_eq!(with_routing(&low, &[], Some("none")), None);
        assert_eq!(pyyaml::dump(&with_routing(&medium, &[], Some("none")).unwrap()), format!("{EU}{LOW}"));
        // a level above that only a person writes, another model or another server: untouched
        for text in [
            format!("{EU}agent:\n  reasoning_effort: high\n"),
            EU.replace("glm-5.3-flash", "deepseek/deepseek-v4.1-flash"),
            EU.replace("https://api.eurouter.ai/api/v1", "http://sparks:8000/v1"),
            format!("{}agent:\n  reasoning_effort: none\n", EU.replace("https://api.eurouter.ai/api/v1", "http://sparks:8000/v1")),
        ] {
            assert_eq!(with_routing(&pyyaml::load(&text).unwrap(), &[], None), None, "{text}");
        }
    }

    #[test]
    fn the_bridges_own_calls_to_glm_ask_for_low() {
        let mut body = json!({"model": "glm-5.3-flash", "max_tokens": 20});
        add_to_body(&order(), "https://api.eurouter.ai/api/v1", &mut body);
        assert_eq!(body["reasoning_effort"], "low");
        assert_eq!(body["provider"]["allow_fallbacks"], true); // all three down: EUrouter still answers
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
        let routed = with_routing(&pyyaml::load(&text).unwrap(), &order(), None).unwrap();
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
            assert_eq!(with_routing(&pyyaml::load(text).unwrap(), &order(), None), None, "{text}");
        }
    }
}
