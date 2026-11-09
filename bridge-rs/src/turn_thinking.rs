//! Automatic thinking (Alex, 9/11: a reply that corrects itself mid-way, « … non attends, sur cet exemple Fabien n'a
//! encore rien payé… »): the model thinks before answering only on the turns that need it (a calculation, a plan of
//! several steps, a comparison, a riddle), and answers at once on the others (speed, cost). Decided per turn by a
//! cheap rule on the person's words, no model call ([`needs_thinking`]); a phone may send its own decision with the
//! message (`thinking` of threads/send.rs `NewMessage`: the on-phone decider studied on branch voice-decider-spike),
//! which wins over the rule.
//!
//! Only for an agent whose level the person never chose (agents_admin::chosen_thinking): a chosen level stays the
//! level of every turn, as before. The level of a thinking turn is Hermes' default (« medium »); a turn without is
//! thinking off. GLM through EUrouter stays on its floor « low » for every turn (agents_admin::keeps_thinking_on:
//! « none » makes it answer nothing; the bench: « medium » doubled its time on calculations for no better result, and
//! a plan with numbers took 40-65 s of checks). Sent as the run's `model_options` (Hermes
//! `_request_reasoning_config`).
//!
//! Never an empty turn: Hermes' empty-answer ladder (patch 0010) asks again without thinking, then without tools, in
//! the same turn; and when a thinking turn still ends without an answer (`record_no_reply`, runs.rs), the agent's
//! next turns don't think for [`NO_REPLY_PAUSE`]. The reasoning text never becomes the reply (Hermes keeps it apart:
//! « Détail du travail »).
//!
//! Bench 2026-11-09 (GLM-5.3 Flash, DeepSeek V4.1 Flash, Alex's Spark): internal/notes/reply-no-selfcorrect.md.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::{json, Value};

use crate::config::{AgentConfig, Config};

/// After a thinking turn that ended without an answer, the agent's next turns answer without thinking this long.
pub const NO_REPLY_PAUSE: Duration = Duration::from_secs(30 * 60);

/// The level of a thinking turn (Hermes' own default for a custom server).
pub const THINKING_TURN: &str = crate::agents_admin::DEFAULT_THINKING;

/// A calculation: two numbers with an operator between them (« 137/3 », « 20 % de 45 », « 3 x 12 »).
static ARITHMETIC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d\s*(?:[+*/×÷=^x]|\s-)\s*\d|\d\s*[%‰]").expect("valid regex"));

/// Words that ask for a calculation, a conversion, date arithmetic, a plan, a comparison or a reasoning, in the
/// apps' languages (fr, en, de, es, nl, it). Matched on the lowercased words, at a word's start.
static ASKS_TO_THINK: LazyLock<Regex> = LazyLock::new(|| {
    #[rustfmt::skip]
    let stems = [
        // calculations, splits, money
        "calcul", "combien", "divis", "partag", "rembours", "solde", "pourcent", "moyenne", "total", "somme",
        "chacun", "qui doit", "dépens", "multipli", "soustr", "intérêt", "remise", "réduction", "tva", "how much",
        "how many", "comput", "split", "owe ", "owes", "percent", "average", "interest", "discount", "wie viel",
        "wieviel", "berechn", "aufteil", "prozent", "durchschnitt", "cuánto", "cuanto", "divid", "porcentaje",
        "promedio", "hoeveel", "bereken", "procent", "gemiddeld", "quanto", "percentuale",
        // conversions and units
        "convert", "conversion", "umrechn", "omrekenen", "en miles", "en km", "in miles", "in km", "fahrenheit",
        "celsius", "gallon", "pouces", "inches", "feet", "pounds", "ounces",
        // dates
        "quel jour", "dans combien", "jours entre", "what day", "days until", "days between", "weeks until",
        "welcher tag", "qué día", "welke dag",
        // plans, comparisons, reasoning
        "planifi", "organis", "étapes", "plan d", "un plan", "itinéraire", "compar", "versus", "vs ", "lequel",
        "laquelle", "avantages", "inconvénients", "pour et contre", "énigme", "logique", "raisonn", "déduis",
        "step by step", "steps", "plan for", "a plan", "itinerary", "which is better", "which one", "pros and cons",
        "riddle", "puzzle", "logic", "schritt", "vergleich", "planen", "paso a paso", "pasos", "planifica",
        "stappen", "vergelijk", "plannen",
    ];
    let alternatives = stems.iter().map(|s| regex::escape(s)).collect::<Vec<_>>().join("|");
    Regex::new(&format!(r"(?:^|[^\p{{L}}])(?:{alternatives})")).expect("valid regex")
});

/// Whether a turn whose person wrote `words` needs the model to think first: a calculation (sums, splits,
/// conversions, dates, percentages), a plan of several steps, a comparison, tricky logic. Plain chat, a question of
/// fact, a quick task don't. Cheap: two regular expressions on the words, no model call.
pub fn needs_thinking(words: &str) -> bool {
    let text = words.to_lowercase();
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    if ARITHMETIC.is_match(text) {
        return true;
    }
    if ASKS_TO_THINK.is_match(text) {
        return true;
    }
    // several amounts in one message (« Alex a payé 120 €, John 80 €, Fabien rien »): a split to work out
    let amounts = text.split(|c: char| !c.is_ascii_digit()).filter(|n| !n.is_empty()).count();
    amounts >= 3 && text.chars().any(|c| "€$£¥".contains(c))
}

/// The agents whose last thinking turn ended without an answer, and when.
static NO_REPLY: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A turn of `agent` that thought ended without an answer (runs.rs): its next turns answer without thinking for
/// [`NO_REPLY_PAUSE`].
pub fn record_no_reply(agent: &str) {
    NO_REPLY.lock().unwrap_or_else(|e| e.into_inner()).insert(agent.to_string(), Instant::now());
}

/// The runs started as thinking turns that haven't ended yet (bounded: a run whose end never came is dropped first).
static THINKING_RUNS: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));
const THINKING_RUNS_MAX: usize = 256;

/// Run `run_id` was started as a thinking turn (threads/send.rs, msg_queue.rs).
pub fn thinking_run(run_id: &str) {
    let mut runs = THINKING_RUNS.lock().unwrap_or_else(|e| e.into_inner());
    if runs.len() >= THINKING_RUNS_MAX {
        runs.remove(0);
    }
    runs.push(run_id.to_string());
}

/// Run `run_id` of `agent` ended (runs.rs, its `run.completed`), `answered` whether it said something: a thinking
/// turn without an answer starts the agent's pause without thinking ([`record_no_reply`]).
pub fn run_ended(agent: &str, run_id: &str, answered: bool) {
    let was_thinking = {
        let mut runs = THINKING_RUNS.lock().unwrap_or_else(|e| e.into_inner());
        let at = runs.iter().position(|r| r == run_id);
        at.map(|i| runs.remove(i)).is_some()
    };
    if was_thinking && !answered {
        tracing::warn!(agent, run_id, "thinking turn without an answer: next turns without thinking");
        record_no_reply(agent);
    }
}

/// Whether `agent`'s turns are in their pause without thinking ([`record_no_reply`]).
pub fn paused(agent: &str) -> bool {
    let mut map = NO_REPLY.lock().unwrap_or_else(|e| e.into_inner());
    match map.get(agent) {
        Some(at) if at.elapsed() < NO_REPLY_PAUSE => true,
        Some(_) => {
            map.remove(agent);
            false
        }
        None => false,
    }
}

/// The `model_options` of a thinking turn and of a turn without, for a model with GLM's floor or not (always its
/// floor: see the module's doc).
pub fn options(think: bool, floor: bool) -> Value {
    match (think, floor) {
        (_, true) => json!({"reasoning": {"enabled": true, "effort": crate::agents_admin::GLM_THINKING}}),
        (true, false) => json!({"reasoning": {"enabled": true, "effort": THINKING_TURN}}),
        (false, false) => json!({"reasoning": {"enabled": false}}),
    }
}

/// Whether this turn thinks: the phone's decision when it sent one, else the rule; never during a pause after a
/// thinking turn without an answer.
pub fn decide(agent: &str, words: &str, phone: Option<bool>) -> bool {
    !paused(agent) && phone.unwrap_or_else(|| needs_thinking(words))
}

/// The run's `model_options` for a turn of `agent` whose person wrote `words` (`phone`: the phone's decision), or
/// None: the agent's own level (the person chose one, or its model is not one the bridge sets: an agent set up by
/// hand, a provider of its own).
pub fn turn_options(config: &Config, agent: &AgentConfig, words: &str, phone: Option<bool>) -> Option<Value> {
    let auto = crate::agents_admin::auto_thinking(config, agent)?;
    let think = decide(&agent.name, words, phone);
    tracing::debug!(agent = %agent.name, think, "turn thinking");
    Some(options(think, auto.floor))
}

/// Whether a run's `model_options` asked for thinking above GLM's floor (a thinking turn of [`options`]).
pub fn thought(options: Option<&Value>) -> bool {
    options
        .and_then(|o| o.get("reasoning"))
        .is_some_and(|r| r.get("enabled") != Some(&json!(false)) && r.get("effort") == Some(&json!(THINKING_TURN)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculations_plans_and_comparisons_think() {
        for words in [
            "Alex a payé 120 €, John 80 € et Fabien rien : qui doit combien à qui ?",
            "On partage les frais du week-end entre nous trois",
            "137/3 ?",
            "ça fait 20 % de 45 ?",
            "Convertis 26 miles en km",
            "Quel jour serons-nous dans 45 jours ?",
            "Combien de jours jusqu'au 25 décembre ?",
            "Fais-moi un plan en 3 étapes pour ranger le garage",
            "Compare l'iPhone 17 et le Pixel 11",
            "Lequel est le moins cher au litre ?",
            "How much is 15% tip on 84 dollars?",
            "Split 240 between 4 people",
            "What day is it 100 days from now?",
            "Wie viel sind 30 Prozent von 80?",
            "¿Cuánto es 12 por 7?",
            "Hoeveel dagen tot kerst?",
            "3 x 12",
            "Plein 52 €, péage 18 €, parking 9 € : total ?",
            // the bench's (internal/notes/reply-no-selfcorrect.md)
            "Week-end à Lisbonne : Alex a payé 240 € (logement), John 90 € (courses), Fabien rien. On partage tout à \
             trois. Qui doit combien à qui ?",
            "Resto : Alex a payé 60 €, John 60 €, Fabien 30 €, et Fabien a aussi payé 90 € d'essence. Comment on \
             équilibre ?",
            "On est 4 : Marie a payé 156,40 €, Paul 73,20 €, Léa 0 €, Tom 50 €. Partage égal, qui rembourse qui ?",
            "Le resto fait 87,50 € et on veut laisser 15 % de pourboire, puis diviser par 3. Combien chacun ?",
            "Le semi-marathon fait combien en miles ? Et 26,2 miles en km ?",
            "Il fait 72 °F à Miami, ça fait combien en °C ? Et 350 °F pour le four ?",
            "On part le 23 décembre et on rentre le 6 janvier. Ça fait combien de nuits ? Et quel jour de la semaine \
             on rentre ?",
            "Le voyage est dans 47 jours : quel jour serons-nous, avec le jour de la semaine ?",
            "Fais-moi un plan en 3 étapes pour organiser l'enterrement de vie de garçon de John à Porto (budget 300 € \
             par personne, 6 personnes).",
            "Train : 89 € par personne aller-retour. Voiture : 620 km aller, 6,5 L/100 km, essence à 1,85 €/L, péages \
             45 € à l'aller ; on est 4. Qu'est-ce qui coûte le moins cher ?",
        ] {
            assert!(needs_thinking(words), "{words}");
        }
    }

    #[test]
    fn plain_chat_answers_at_once() {
        for words in [
            "Salut !",
            "Merci beaucoup",
            "Bonne nuit Bip",
            "Raconte-moi une blague",
            "Quelle est la capitale du Portugal ?",
            "Rappelle-moi d'appeler maman demain à 18 h",
            "Hello, how are you?",
            "Écris un petit poème sur l'automne",
            "Je suis fatigué aujourd'hui",
            "C'est quoi la météo à Lyon ?",
            "Salut ! Ça va ?",
            "Merci pour ton aide hier, c'était top",
            "Raconte-moi une blague courte sur les vacances",
            "",
        ] {
            assert!(!needs_thinking(words), "{words}");
        }
    }

    #[test]
    fn the_options_of_each_kind_of_turn() {
        assert_eq!(options(true, false), json!({"reasoning": {"enabled": true, "effort": "medium"}}));
        // GLM through EUrouter: its floor on every turn (« none » makes it answer nothing, « medium » slower only)
        assert_eq!(options(true, true), json!({"reasoning": {"enabled": true, "effort": "low"}}));
        assert_eq!(options(false, false), json!({"reasoning": {"enabled": false}}));
        assert_eq!(options(false, true), json!({"reasoning": {"enabled": true, "effort": "low"}}));
        assert!(thought(Some(&options(true, false))));
        assert!(!thought(Some(&options(true, true))));
        assert!(!thought(Some(&options(false, true))));
        assert!(!thought(Some(&options(false, false))));
        assert!(!thought(None));
    }

    #[test]
    fn the_phone_decides_when_it_says() {
        assert!(decide("t-phone", "Salut", Some(true)));
        assert!(!decide("t-phone", "Combien font 12 x 7 ?", Some(false)));
        assert!(decide("t-phone", "Combien font 12 x 7 ?", None));
    }

    #[test]
    fn automatic_only_when_the_person_never_chose_a_level() {
        use crate::agents_admin::{auto_thinking_of, pyyaml, AutoThinking};
        let eu = "model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n";
        let spark = "model:\n  provider: custom\n  default: GLM-5.3-Flash-EXL3\n  base_url: http://sparks:8888/v1\n";
        let load = |s: &str| pyyaml::load(s).unwrap();
        assert_eq!(auto_thinking_of(&load(eu), None), Some(AutoThinking { floor: true }));
        assert_eq!(auto_thinking_of(&load(spark), None), Some(AutoThinking { floor: false }));
        // Hermes' template « medium » and GLM's floor are not the person's
        let medium = format!("{spark}agent:\n  reasoning_effort: medium\n");
        assert_eq!(auto_thinking_of(&load(&medium), None), Some(AutoThinking { floor: false }));
        let low = format!("{eu}agent:\n  reasoning_effort: low\n");
        assert_eq!(auto_thinking_of(&load(&low), None), Some(AutoThinking { floor: true }));
        // a chosen level stays every turn's
        assert_eq!(auto_thinking_of(&load(spark), Some("none")), None);
        assert_eq!(auto_thinking_of(&load(eu), Some("medium")), None);
        // a level only a person writes, from before the record
        let high = format!("{spark}agent:\n  reasoning_effort: high\n");
        assert_eq!(auto_thinking_of(&load(&high), None), None);
        let none = format!(
            "{spark}agent:\n  reasoning_effort: none\nproviders:\n  bipagents-thinking:\n    api: http://sparks:8888/v1\n"
        );
        assert_eq!(auto_thinking_of(&load(&none), None), None);
        // a model the bridge doesn't set
        assert_eq!(auto_thinking_of(&load("model:\n  provider: openrouter\n  default: x\n"), None), None);
    }

    #[test]
    fn a_thinking_run_that_ends_without_an_answer_pauses_its_agent() {
        thinking_run("run-a");
        run_ended("t-ended", "run-a", true);
        assert!(!paused("t-ended"), "answered");
        run_ended("t-ended", "run-b", false);
        assert!(!paused("t-ended"), "not a thinking turn");
        thinking_run("run-c");
        run_ended("t-ended", "run-c", false);
        assert!(paused("t-ended"));
        assert!(!THINKING_RUNS.lock().unwrap().iter().any(|r| r == "run-a" || r == "run-c"));
    }

    #[test]
    fn a_thinking_turn_without_an_answer_pauses_thinking() {
        assert!(decide("t-pause", "137/3 ?", None));
        record_no_reply("t-pause");
        assert!(paused("t-pause"));
        assert!(!decide("t-pause", "137/3 ?", None));
        assert!(!decide("t-pause", "137/3 ?", Some(true)), "the pause wins over the phone too");
        assert!(decide("t-other", "137/3 ?", None), "another agent still thinks");
        // the pause ends
        NO_REPLY.lock().unwrap().insert("t-pause".into(), Instant::now() - NO_REPLY_PAUSE - Duration::from_secs(1));
        assert!(!paused("t-pause"));
        assert!(decide("t-pause", "137/3 ?", None));
    }
}
