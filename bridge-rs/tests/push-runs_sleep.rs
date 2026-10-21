//! A place on a server the app created sleeps when idle and wakes at the next request (Python: tests/test_sleep.py).

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};

use bipbridge::config::{Config, SharedConfig};
use bipbridge::sleep::{self, Sleep};
use common::*;
use futures::FutureExt;

struct Systemd {
    running: Mutex<bool>,
    calls: Mutex<Vec<Vec<String>>>,
}

struct Place {
    sleep: Arc<Sleep>,
    clock: Arc<Mutex<f64>>,
    systemd: Arc<Systemd>,
}

fn a_place(config: &Config, home: &Path, routines: Vec<String>, cloud: bool) -> Place {
    let clock = Arc::new(Mutex::new(1_790_000_000.0_f64));
    let systemd = Arc::new(Systemd { running: Mutex::new(true), calls: Mutex::new(Vec::new()) });
    let now = clock.clone();
    let sd = systemd.clone();
    let run: sleep::Run = Arc::new(move |args: Vec<String>| {
        let sd = sd.clone();
        async move {
            sd.calls.lock().unwrap().push(args.clone());
            match args[0].as_str() {
                "list-units" => {
                    return Ok("hermes-gateway.service loaded active running Hermes\n\
                               bipagents-bridge.service loaded active running Bridge\n"
                        .to_string())
                }
                "stop" => *sd.running.lock().unwrap() = false,
                "start" => *sd.running.lock().unwrap() = true,
                _ => {}
            }
            Ok(String::new())
        }
        .boxed()
    });
    let ran: sleep::Routines = Arc::new(move || {
        let routines = routines.clone();
        async move { routines }.boxed()
    });
    let sd = systemd.clone();
    let up: sleep::HermesUp = Arc::new(move || {
        let sd = sd.clone();
        async move { *sd.running.lock().unwrap() }.boxed()
    });
    let mut config = config.clone();
    config.cloud = cloud;
    let clock_fn: bipbridge::py::Clock = Arc::new(move || *now.lock().unwrap());
    let sleep = Arc::new(Sleep::with(SharedConfig::new(config), ran, up, run, clock_fn, Some(home.to_path_buf())));
    Place { sleep, clock, systemd }
}

#[tokio::test]
async fn an_idle_place_sleeps_then_the_next_request_wakes_it() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let home = dir.path().join("home");
    let place = a_place(&config, &home, vec![], true);
    assert!(!place.sleep.check().await); // used today
    *place.clock.lock().unwrap() += 15.0 * 86400.0;
    assert!(place.sleep.check().await);
    assert!(!*place.systemd.running.lock().unwrap());
    let calls = place.systemd.calls.lock().unwrap().clone();
    assert!(calls.contains(&vec!["stop".to_string(), "hermes-gateway.service".to_string()]));
    // the bridge keeps answering
    assert!(!calls.contains(&vec!["stop".to_string(), "bipagents-bridge.service".to_string()]));
    assert!(home.join(sleep::MARKER).is_file()); // the seat agent says « asleep »
    place.sleep.awake().await;
    assert!(*place.systemd.running.lock().unwrap());
    assert!(!place.sleep.asleep());
    assert!(!home.join(sleep::MARKER).exists());
    // Remembered across restarts of the bridge.
    let s = &place.sleep;
    let again = Sleep::with(
        s.config.clone(),
        s.routines.clone(),
        s.hermes_up.clone(),
        s.run.clone(),
        s.now.clone(),
        Some(home.clone()),
    );
    assert_eq!(again.last(), *place.clock.lock().unwrap());
    assert!(!again.asleep());
    // sleep.json, as Python writes it
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("data").join("sleep.json")).unwrap()).unwrap();
    assert_eq!(saved, serde_json::json!({"last": *place.clock.lock().unwrap(), "asleep": false}));
}

#[tokio::test]
async fn a_place_with_routines_or_at_home_never_sleeps() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), &Urls::default());
    let recent = (chrono::Utc::now() - chrono::Duration::days(2)).to_rfc3339();
    let place = a_place(&config, dir.path(), vec![recent.clone()], true);
    *place.clock.lock().unwrap() += 30.0 * 86400.0;
    assert!(!place.sleep.check().await);
    assert!(*place.systemd.running.lock().unwrap());
    let home = a_place(&config, dir.path(), vec![recent], false);
    *home.clock.lock().unwrap() += 30.0 * 86400.0;
    assert!(!home.sleep.enabled());
    assert!(!home.sleep.check().await);
}
