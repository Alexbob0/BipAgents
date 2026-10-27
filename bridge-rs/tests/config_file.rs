//! The bridge's config file can't be broken by writers running at once, nor by a write that loses what it holds, and a
//! broken one is put back from its last good copy at start (config.rs, « the config file on disk »). aibox,
//! 2026-10-26: debris at the top of bridge.toml, then the whole file replaced by four agents' voices.

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bipbridge::agents_admin::{append_agent, remove_agent_section};
use bipbridge::config::{load_config, load_config_at_start, restored, update_source, AgentConfig};
use bipbridge::voices::set_section_keys;
use serde_json::json;

const AGENTS: usize = 12;

/// A config that loads, with `AGENTS` agents.
fn config_text() -> String {
    let mut text = format!("# keys inside: chmod 600.\nbridge_key = \"{}\"\nport = 9210\n", "k".repeat(40));
    for i in 0..AGENTS {
        text.push_str(&format!(
            "\n[agents.a{i}]\ndisplay_name = \"A{i}\"\nhermes_url = \"http://127.0.0.1:{}\"\nhermes_key = \"h{i}\"\n",
            8600 + i
        ));
    }
    text
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn beside(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", path.display()))
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// The file parses and keeps its key and every agent's address.
fn assert_whole(text: &str) -> toml::Table {
    let data: toml::Table = text.parse().unwrap_or_else(|e| panic!("not TOML any more ({e}):\n{text}"));
    assert_eq!(data["bridge_key"].as_str(), Some("k".repeat(40).as_str()), "{text}");
    for i in 0..AGENTS {
        let agent = &data["agents"][&format!("a{i}")];
        assert_eq!(agent["hermes_url"].as_str(), Some(format!("http://127.0.0.1:{}", 8600 + i).as_str()), "{text}");
    }
    data
}

/// Run by `many_writers_in_threads_and_processes_keep_the_file_whole` in processes of its own (`HAMMER_FILE`).
#[test]
#[ignore]
fn hammer_child() {
    let (Ok(file), Ok(id)) = (std::env::var("HAMMER_FILE"), std::env::var("HAMMER_ID")) else { return };
    for round in 0..30 {
        let values = [("voice".to_string(), Some(json!(format!("pocket:p{id}-{round}"))))];
        set_section_keys(&file, &format!("agents.a{}", id.parse::<usize>().unwrap() % AGENTS), &values).unwrap();
        set_section_keys(&file, &format!("process{id}"), &[("round".to_string(), Some(json!(round)))]).unwrap();
    }
}

#[test]
fn many_writers_in_threads_and_processes_keep_the_file_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    write(&path, &config_text());
    let p = path.to_str().unwrap().to_string();
    let children: Vec<_> = (0..2)
        .map(|id| {
            Command::new(std::env::current_exe().unwrap())
                .args(["hammer_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
                .env("HAMMER_FILE", &p)
                .env("HAMMER_ID", (100 + id).to_string())
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let (p, done) = (p.clone(), done.clone());
        std::thread::spawn(move || {
            let mut reads = 0;
            while !done.load(Ordering::Relaxed) {
                assert_whole(&std::fs::read_to_string(&p).unwrap());
                reads += 1;
            }
            reads
        })
    };
    let voices: Vec<_> = (0..AGENTS)
        .map(|i| {
            let p = p.clone();
            std::thread::spawn(move || {
                for round in 0..10 {
                    let values = [("voice".to_string(), Some(json!(format!("pocket:v{i}-{round}"))))];
                    set_section_keys(&p, &format!("agents.a{i}"), &values).unwrap();
                }
            })
        })
        .collect();
    let comings_and_goings: Vec<_> = (0..3)
        .map(|t| {
            let p = p.clone();
            std::thread::spawn(move || {
                for round in 0..8 {
                    let agent = AgentConfig {
                        name: format!("extra{t}x{round}"),
                        display_name: "Extra".into(),
                        hermes_url: "http://127.0.0.1:9000".into(),
                        hermes_key: "x".into(),
                        ..AgentConfig::default()
                    };
                    append_agent(&p, &agent, &[]).unwrap();
                    remove_agent_section(&p, &agent.name).unwrap();
                }
            })
        })
        .collect();
    for t in voices.into_iter().chain(comings_and_goings) {
        t.join().unwrap();
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    done.store(true, Ordering::Relaxed);
    assert!(reader.join().unwrap() > 0);
    let text = std::fs::read_to_string(&path).unwrap();
    let data = assert_whole(&text);
    for i in 0..AGENTS {
        let voice = data["agents"][&format!("a{i}")]["voice"].as_str().unwrap().to_string();
        assert!(voice.starts_with("pocket:"), "{voice}");
    }
    assert_eq!(data["agents"].as_table().unwrap().len(), AGENTS, "every extra agent removed: {text}");
    assert_eq!(data["process100"]["round"].as_integer(), Some(29));
    assert_eq!(data["process101"]["round"].as_integer(), Some(29));
    assert!(text.starts_with("# keys inside: chmod 600."));
    assert_eq!(mode(&path), 0o600);
    assert_eq!(std::fs::read_to_string(beside(&path, ".last-good")).unwrap(), text);
    assert_eq!(mode(&beside(&path, ".last-good")), 0o600);
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_write_waits_for_the_installers_lock() {
    // installer/bipinstall/bridgeconf.py takes `flock` on bridge.toml.lock: the bridge's writes wait for it, and read
    // the file once it is released (never the text from before the installer's change).
    let Ok(python) = which("python3") else { return };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    write(&path, &config_text());
    let script = r#"
import fcntl, os, sys, time
path = sys.argv[1]
lock = open(path + ".lock", "a+")
fcntl.flock(lock, fcntl.LOCK_EX)
print("locked", flush=True)
text = open(path).read()
time.sleep(0.6)
tmp = path + ".py.tmp"
with open(tmp, "w") as fh:
    fh.write(text + '\n[agents.frompython]\nhermes_url = "http://127.0.0.1:9999"\n')
os.chmod(tmp, 0o600)
os.replace(tmp, path)
fcntl.flock(lock, fcntl.LOCK_UN)
"#;
    let mut child = Command::new(python).args(["-c", script, path.to_str().unwrap()]).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "locked");
    let started = Instant::now();
    set_section_keys(path.to_str().unwrap(), "agents.a0", &[("voice".into(), Some(json!("pocket:galet")))]).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300), "the write didn't wait for the lock");
    assert!(child.wait().unwrap().success());
    let data = assert_whole(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(data["agents"]["frompython"]["hermes_url"].as_str(), Some("http://127.0.0.1:9999"));
    assert_eq!(data["agents"]["a0"]["voice"].as_str(), Some("pocket:galet"));
}

fn which(program: &str) -> Result<PathBuf, ()> {
    let paths = std::env::var_os("PATH").ok_or(())?;
    std::env::split_paths(&paths).map(|d| d.join(program)).find(|p| p.is_file()).ok_or(())
}

/// What the aibox's file became on 2026-10-26 at 21:43 (201 bytes, keys and addresses gone).
const INCIDENT: &str = "[agents.vie]\nvoice = \"pocket:alba\"\n\n[agents.chi]\nvoice = \"pocket:colibri\"\n\n\
[agents.kerfy-studio]\nvoice = \"pocket:azur\"\n\n[agents.coach]\nvoice = \"pocket:sable\"\ngalet\"\n";

#[test]
fn a_write_that_loses_what_the_file_holds_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    let p = path.to_str().unwrap();
    write(&path, &config_text());
    let refused = |change: &dyn Fn(&str) -> String| {
        let err = update_source(p, |text| Ok(change(text))).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), config_text(), "the file stays as it was");
        err.to_string()
    };
    assert!(refused(&|_| INCIDENT.to_string()).contains("isn't valid TOML"));
    assert!(refused(&|t| format!("e: chmod 600.\n{t}")).contains("isn't valid TOML"));
    assert!(refused(&|t| t.replace(&format!("bridge_key = \"{}\"\n", "k".repeat(40)), "")).contains("bridge_key"));
    assert!(refused(&|t| t.replace("hermes_url = \"http://127.0.0.1:8603\"\n", "")).contains("agent a3"));
    assert!(refused(&|t| t.split("\n[agents.a5]").next().unwrap().to_string() + "\n").contains("agent a5"));
    // TOML that the bridge wouldn't load (it did before)
    assert!(refused(&|t| format!("{t}\n[apns]\nenvironment = \"staging\"\n")).contains("couldn't load"));
    // the agent being deleted may go, and only it
    remove_agent_section(p, "a5").unwrap();
    let data: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert!(data["agents"].get("a5").is_none());
    assert_eq!(data["agents"].as_table().unwrap().len(), AGENTS - 1);
}

#[test]
fn a_broken_file_is_put_back_from_its_last_good_copy_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    let p = path.to_str().unwrap();
    write(&path, &config_text());
    assert_eq!(load_config_at_start(Some(p)).unwrap().agents.len(), AGENTS);
    let good = beside(&path, ".last-good");
    assert_eq!(std::fs::read_to_string(&good).unwrap(), config_text());
    assert_eq!(mode(&good), 0o600);

    // the second break of 2026-10-26: written by something that doesn't take the lock
    write(&path, INCIDENT);
    assert!(load_config(Some(p)).is_err());
    let config = load_config_at_start(Some(p)).unwrap();
    assert_eq!(config.agents.len(), AGENTS);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), config_text());
    assert_eq!(mode(&path), 0o600);
    let broken: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("bridge.toml.broken-"))
        .collect();
    assert_eq!(broken.len(), 1);
    assert_eq!(std::fs::read_to_string(broken[0].path()).unwrap(), INCIDENT, "the broken file is kept");
    assert_eq!(std::fs::read_to_string(&good).unwrap(), config_text(), "the last good copy stays");
    // said in /health (the other tests here restore too, in parallel: the latest restore is what it says)
    assert!(restored().is_some_and(|said| said["at"].is_number()));
}

#[test]
fn a_file_gone_or_emptied_of_its_agents_is_put_back_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    let p = path.to_str().unwrap();
    write(&path, &config_text());
    load_config_at_start(Some(p)).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(load_config_at_start(Some(p)).unwrap().agents.len(), AGENTS);
    // loads, but every agent the last good copy had is gone (cut after its first lines)
    write(&path, &format!("bridge_key = \"{}\"\nport = 9210\n", "k".repeat(40)));
    assert_eq!(load_config_at_start(Some(p)).unwrap().agents.len(), AGENTS);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), config_text());
}

#[test]
fn without_a_usable_copy_the_error_names_the_copies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    let p = path.to_str().unwrap();
    // no last good copy: today's error, nothing else
    write(&path, INCIDENT);
    let expected = load_config(Some(p)).unwrap_err();
    assert_eq!(load_config_at_start(Some(p)).unwrap_err(), expected);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), INCIDENT, "left as it is");
    // a last good copy that doesn't load either
    write(&beside(&path, ".last-good"), "bridge_key = \n");
    let err = load_config_at_start(Some(p)).unwrap_err().0;
    assert!(err.contains("last good copy can't be used either"), "{err}");
    assert!(err.contains(&format!("{}.last-good", path.display())), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), INCIDENT, "left as it is");
}

#[test]
fn a_file_that_does_not_load_never_becomes_the_last_good_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bridge.toml");
    let p = path.to_str().unwrap();
    write(&path, &config_text());
    load_config_at_start(Some(p)).unwrap();
    // valid TOML the bridge can't load, set by hand: a change on top of it is written, the last good copy stays
    let unloadable = format!("{}\n[apns]\nenvironment = \"staging\"\n", config_text());
    write(&path, &unloadable);
    set_section_keys(p, "agents.a1", &[("voice".into(), Some(json!("pocket:alba")))]).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("pocket:alba"));
    assert_eq!(std::fs::read_to_string(beside(&path, ".last-good")).unwrap(), config_text());
}
