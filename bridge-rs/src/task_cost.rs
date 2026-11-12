//! What a background task costs, in euros, counted as the AI credit counts it (Alex, 2026-11-04): a task stopped at
//! « 5 % of the month » while his credit had lost under 3 %. Until then a task's budget was tokens, every token of
//! the prompt counted at the full input price at the `[notebook]` prices (0.50 / 1.50 € per million, « on the high
//! side »), while the router bills the prompt's cache reads at a fraction (GLM-5.3 Flash: 0.10 € fresh, 0.02 € from
//! the cache, 0.35 € out) and the month's credit is the router's own count of the place's key (guichet
//! credit_pool.rs). A task's work reads its whole conversation again at each step, nearly all of it from the cache:
//! counted in full it was about two and a half times what the credit lost.
//!
//! Now the task's euros are its calls' real tokens (Hermes patch 0020: fresh input, cache reads, output) at the
//! router's prices for the agent's model, the list the guichet serves the apps (`GET /v1/models?region=`, the same
//! numbers its credit is metered against): fresh input × input price + cache reads × cached price + output × output
//! price. Its budget is in euros (`budget_eur`), its spend too (`eur`); the hub stops its run before a model call
//! that would pass what is left (runs.rs `limit_cost`).
//!
//! The prices: on a place of the offer, the guichet's list (read once an hour, the agent's model or the region's
//! default); elsewhere, or the guichet away, `[jobs] price_in / price_cached / price_out` when set, else the
//! `[notebook]` prices.
//!
//! A home install (its own model, the apps show tokens there): a cache read counts `HOME_CACHE_WEIGHT` (10 %) of a
//! fresh input token unless `[jobs] price_cached` says otherwise (Alex, 2026-11-12: his task asked « Continuer » again
//! and again, each step's whole conversation, read again from the cache, counted in full). Why 10 %: a local server
//! (llama.cpp's prompt cache, vLLM's / SGLang's prefix caching, Ollama keeping the context) skips the prefill of a
//! prefix it holds, the costly part of reading a prompt; what is left is reading the KV cache back, a small fraction;
//! the providers a home install may use instead bill a cache read 10 % (Anthropic, DeepSeek, OpenAI's recent models)
//! to 25 % of fresh input. A place of the offer whose guichet is away keeps the cache at the input price (on the
//! high side, as before: there it is the person's credit).

use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};
use crate::runs::RunUsage;
use crate::Dict;

/// How long the guichet's list is kept.
pub const REFRESH_SECONDS: f64 = 3600.0;

/// Euros per million tokens: fresh input, input read from the router's cache, output.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input: f64,
    pub cached: f64,
    pub output: f64,
}

impl Prices {
    /// What `input` tokens (cache reads included, as Hermes counts them), `cached` of them read from the cache, and
    /// `output` tokens cost.
    pub fn cost(&self, input: i64, cached: i64, output: i64) -> f64 {
        let (input, output) = (input.max(0), output.max(0));
        let cached = cached.clamp(0, input);
        ((input - cached) as f64 * self.input + cached as f64 * self.cached + output as f64 * self.output) / 1e6
    }

    /// A run's calls so far.
    pub fn of_usage(&self, usage: &RunUsage) -> f64 {
        self.cost(usage.input, usage.cached, usage.output)
    }

    /// What the run's next call costs at least: it reads what the last one read (from the cache in the same share),
    /// its answer left out.
    pub fn next_call(&self, usage: &RunUsage) -> f64 {
        let share = if usage.input > 0 { (usage.cached as f64 / usage.input as f64).clamp(0.0, 1.0) } else { 0.0 };
        let last = usage.last_input.max(0) as f64;
        (last * (1.0 - share) * self.input + last * share * self.cached) / 1e6
    }

    /// A task's tokens (`{"input", "output", "cached"}`).
    pub fn of_tokens(&self, tokens: Option<&Value>) -> f64 {
        let n = |k: &str| tokens.and_then(|t| t.get(k)).and_then(|v| crate::py::py_int(v).ok()).unwrap_or(0);
        self.cost(n("input"), n("cached"), n("output"))
    }

    /// Tokens of an agent's work for `euros` when none come from the cache (95 % read, 5 % written): what a home
    /// install's screens and its agent are told in tokens.
    pub fn tokens_for(&self, euros: f64) -> i64 {
        let per = (0.95 * self.input + 0.05 * self.output) / 1e6;
        if per <= 0.0 {
            return 0;
        }
        (euros.max(0.0) / per) as i64
    }

    pub fn to_value(self) -> Value {
        json!({"input": self.input, "cached": self.cached, "output": self.output})
    }

    /// The prices a task keeps (`job["prices"]`).
    pub fn of_job(job: &Dict) -> Option<Prices> {
        let p = job.get("prices")?.as_object()?;
        let n = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
        Some(Prices { input: n("input")?, cached: n("cached")?, output: n("output")? })
    }
}

/// What a cache read counts on a home install, as a share of a fresh input token (the module's doc).
pub const HOME_CACHE_WEIGHT: f64 = 0.10;

/// The install's own prices (the module's doc), on a place of the offer or not.
pub fn configured(config: &Config) -> Prices {
    configured_on(config, crate::handover::on_offer())
}

/// `configured`: `[jobs] price_*` when set, else the `[notebook]` ones; cache reads at `[jobs] price_cached` when set,
/// else at `HOME_CACHE_WEIGHT` of the input price at home, at the input price on the offer.
pub fn configured_on(config: &Config, offer: bool) -> Prices {
    let weight = if offer { 1.0 } else { HOME_CACHE_WEIGHT };
    let j = &config.jobs;
    if j.price_in > 0.0 || j.price_out > 0.0 {
        let cached = if j.price_cached > 0.0 { j.price_cached } else { j.price_in * weight };
        return Prices { input: j.price_in, cached, output: j.price_out };
    }
    let n = &config.notebook;
    Prices { input: n.price_in.max(0.0), cached: n.price_in.max(0.0) * weight, output: n.price_out.max(0.0) }
}

/// One model's prices in the guichet's list (`input_eur_per_mtok`, `cached_eur_per_mtok`, `output_eur_per_mtok`);
/// `model` absent or unlisted: the list's default.
pub fn from_catalogue(list: &Value, model: Option<&str>) -> Option<Prices> {
    let models = list.get("models")?.as_array()?;
    let find = |id: &str| models.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(id));
    let item = model
        .and_then(|m| find(m).or_else(|| find(m.rsplit('/').next().unwrap_or(m))))
        .or_else(|| list.get("default").and_then(Value::as_str).and_then(find))?;
    let n = |k: &str| item.get(k).and_then(Value::as_f64).filter(|v| v.is_finite() && *v >= 0.0);
    let input = n("input_eur_per_mtok")?;
    Some(Prices { input, cached: n("cached_eur_per_mtok").unwrap_or(input), output: n("output_eur_per_mtok")? })
}

static CATALOGUE: LazyLock<Mutex<Option<(f64, Value)>>> = LazyLock::new(|| Mutex::new(None));
static HTTP: LazyLock<reqwest::Client> =
    LazyLock::new(|| reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap_or_default());

async fn catalogue(config: &Config) -> Option<Value> {
    let now = crate::py::time_time();
    let held = CATALOGUE.lock().ok().and_then(|c| c.clone());
    if let Some((at, list)) = &held {
        if now - at < REFRESH_SECONDS {
            return Some(list.clone());
        }
    }
    let url = format!("{}/v1/models", crate::templates::guichet_url(config));
    let read = HTTP.get(url).query(&[("region", config.jobs.region.as_str())]).send().await;
    match read {
        Ok(resp) if resp.status().is_success() => {
            let list = resp.json::<Value>().await.ok()?;
            if let Ok(mut c) = CATALOGUE.lock() {
                *c = Some((now, list.clone()));
            }
            Some(list)
        }
        _ => {
            tracing::info!("the guichet's model prices unread: the last ones, else the install's");
            held.map(|(_, list)| list)
        }
    }
}

/// The prices `agent`'s task is counted at (the module's doc).
pub async fn for_agent(config: &Config, agent: &AgentConfig) -> Prices {
    if crate::handover::on_offer() {
        if let Some(list) = catalogue(config).await {
            let model = crate::health::model_of(config, agent);
            if let Some(prices) = from_catalogue(&list, model.as_deref()) {
                return prices;
            }
        }
    }
    configured(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLM: Prices = Prices { input: 0.10, cached: 0.02, output: 0.35 };

    #[test]
    fn cache_reads_cost_what_the_router_bills() {
        // Alex's task of 2026-11-04 in its shape: 1.5 M read, 1.35 M of it from the cache, 30 k written
        let real = GLM.cost(1_500_000, 1_350_000, 30_000);
        assert!((real - (0.015 + 0.027 + 0.0105)).abs() < 1e-9, "{real}");
        // what the old count said for the same tokens (everything at 0.50 / 1.50, no cache): fifteen times more
        let old = configured_on(&Config::with_key("k"), true).cost(1_500_000, 1_350_000, 30_000);
        assert!((old - 0.795).abs() < 1e-9, "{old}");
        assert_eq!(GLM.cost(10, 50, -3), 10.0 * 0.02 / 1e6, "cache reads never more than the input, nothing negative");
    }

    #[test]
    fn the_next_call_at_the_last_ones_cache_share() {
        let usage = RunUsage { calls: 3, input: 100_000, output: 2_000, cached: 90_000, last_input: 40_000 };
        assert!((GLM.of_usage(&usage) - (0.001 + 0.0018 + 0.0007)).abs() < 1e-9);
        assert!((GLM.next_call(&usage) - (4_000.0 * 0.10 + 36_000.0 * 0.02) / 1e6).abs() < 1e-12);
    }

    #[test]
    fn prices_from_the_guichet_s_list() {
        let list = json!({"default": "glm-5.3-flash", "models": [
            {"id": "glm-5.3-flash", "input_eur_per_mtok": 0.1, "cached_eur_per_mtok": 0.02, "output_eur_per_mtok": 0.35},
            {"id": "deepseek/deepseek-v4.1-flash", "input_eur_per_mtok": 0.25, "output_eur_per_mtok": 0.9},
            {"id": "bare"}]});
        assert_eq!(from_catalogue(&list, Some("glm-5.3-flash")), Some(GLM));
        assert_eq!(from_catalogue(&list, None), Some(GLM), "the default");
        assert_eq!(from_catalogue(&list, Some("unknown")), Some(GLM), "unlisted: the default");
        assert_eq!(
            from_catalogue(&list, Some("deepseek/deepseek-v4.1-flash")),
            Some(Prices { input: 0.25, cached: 0.25, output: 0.9 }),
            "no cache price: the input's"
        );
        assert_eq!(from_catalogue(&list, Some("bare")), None);
        assert_eq!(from_catalogue(&json!({}), None), None);
    }

    #[test]
    fn the_install_s_prices() {
        let mut config = Config::with_key("k");
        // a place of the offer whose guichet is away: as before, cache reads at the input price
        assert_eq!(configured_on(&config, true), Prices { input: 0.50, cached: 0.50, output: 1.50 });
        // a home install: a cache read counts a tenth of a fresh input token
        assert_eq!(configured_on(&config, false), Prices { input: 0.50, cached: 0.05, output: 1.50 });
        config.jobs.price_in = 0.1;
        config.jobs.price_out = 0.35;
        assert_eq!(configured_on(&config, true), Prices { input: 0.1, cached: 0.1, output: 0.35 });
        let home = configured_on(&config, false);
        assert!((home.cached - 0.01).abs() < 1e-12 && home.input == 0.1 && home.output == 0.35, "{home:?}");
        config.jobs.price_cached = 0.02;
        assert_eq!(configured_on(&config, true), GLM);
        assert_eq!(configured_on(&config, false), GLM, "[jobs] price_cached wins at home too");
        assert_eq!(configured_on(&Config::with_key("k"), false).tokens_for(0.20), 363_636);
        let job = json!({"prices": GLM.to_value()});
        assert_eq!(Prices::of_job(job.as_object().unwrap()), Some(GLM));
    }

    #[test]
    fn a_long_task_at_home_counts_its_cache_reads_lightly() {
        // Alex, 2026-11-12, a home install: 40 steps, each reading its 30 k-token conversation again (28.5 k of it from
        // the cache, 1.5 k new), writing 1 k
        let config = Config::with_key("k");
        let (input, cached, output) = (40 * 30_000, 40 * 28_500, 40 * 1_000);
        let before = configured_on(&config, true).cost(input, cached, output);
        let now = configured_on(&config, false).cost(input, cached, output);
        assert!((before - 0.66).abs() < 1e-9, "{before}"); // 3.3 times the small budget (0.20): three « Continuer »
        assert!((now - (0.03 + 0.057 + 0.06)).abs() < 1e-9, "{now}"); // 0.147: within it
    }
}
