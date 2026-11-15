//! Security review, second pass: the memory fix of E1 holds even for a caller that has a pass (the first test of
//! family_review_memory.rs now passes without one: its calls are refused as « away » before any byte is kept). 32
//! calls of 1 MiB from 32 addresses, each with its own pair of mailboxes and the pass it gives itself, each body sent
//! but its last byte: the relay must hold no more than its room (8 MiB here). Expected to pass on c18c86384.

use std::alloc::{GlobalAlloc, Layout, System};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc(layout);
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let p = System.alloc_zeroed(layout);
        if !p.is_null() {
            LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        System.dealloc(p, layout);
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let q = System.realloc(p, layout, new_size);
        if !q.is_null() {
            LIVE.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        q
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

static ZEROS: [u8; 64 * 1024] = [0u8; 64 * 1024];

fn config(family: FamilyConfig) -> Config {
    Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        seal_keys: Sealer::generate_key(),
        apns: None,
        fcm: None,
        fcm_send_base: None,
        pushes_per_device_hour: 10,
        pushes_per_ip_hour: 10,
        registrations_per_ip_hour: 10,
        trust_forwarded_for: true,
        report_url: None,
        report_secret: None,
        reports_per_ip_hour: 1,
        blocked: Vec::new(),
        guichet_keys: Vec::new(),
        r1_until: 0,
        family,
    }
}

async fn mailbox(http: &reqwest::Client, url: &str) -> (String, String) {
    let body: Value =
        http.post(format!("{url}/v1/family/mailbox")).json(&json!({})).send().await.unwrap().json().await.unwrap();
    (body["mailbox"].as_str().unwrap().to_string(), body["token"].as_str().unwrap().to_string())
}

async fn pass(http: &reqwest::Client, url: &str, token: &str) -> String {
    let body: Value =
        http.post(format!("{url}/v1/family/pass")).bearer_auth(token).send().await.unwrap().json().await.unwrap();
    body["pass"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn calls_with_a_pass_from_many_addresses_hold_no_more_than_the_room() {
    const BODY: usize = 1 << 20;
    const ROOM: usize = 8 << 20;
    const CALLS: usize = 32;
    let family = FamilyConfig {
        request_bytes: BODY,
        queued_bytes: ROOM,
        mailboxes_per_ip_hour: 1000,
        pickup: Duration::from_secs(2),
        poll: Duration::from_secs(30),
        away: Duration::from_secs(10),
        answer_start: Duration::from_secs(5),
        ..FamilyConfig::default()
    };
    let relay = Arc::new(Relay::new(config(family), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    let url = format!("http://{addr}");
    let http = reqwest::Client::new();
    let mut calls = Vec::new();
    let mut polls = Vec::new();
    for _ in 0..CALLS {
        let ((_, caller), (target, target_token)) = (mailbox(&http, &url).await, mailbox(&http, &url).await);
        let pass = pass(&http, &url, &target_token).await;
        let (http2, url2) = (http.clone(), url.clone());
        polls.push(tokio::spawn(async move {
            http2.get(format!("{url2}/v1/family/next")).bearer_auth(target_token).send().await
        }));
        calls.push((caller, target, pass));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = LIVE.load(Ordering::Relaxed);
    let mut held = Vec::new();
    for (i, (caller, target, pass)) in calls.iter().enumerate() {
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        let head = format!(
            "POST /v1/family/call/{target} HTTP/1.1\r\nHost: relay\r\nAuthorization: Bearer {caller}\r\n\
             x-bip-kind: call\r\nx-bip-pass: {pass}\r\nX-Forwarded-For: 2001:db8:{i}::1\r\n\
             Content-Type: application/octet-stream\r\nContent-Length: {BODY}\r\n\r\n"
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        let mut left = BODY - 1;
        while left > 0 {
            let n = left.min(ZEROS.len());
            if socket.write_all(&ZEROS[..n]).await.is_err() {
                break; // refused and closed by the relay: nothing kept
            }
            left -= n;
        }
        held.push(socket);
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let pinned = LIVE.load(Ordering::Relaxed) - before;
    drop(held);
    for poll in polls {
        poll.abort();
    }
    assert!(
        pinned < (2 * ROOM) as isize,
        "{CALLS} calls with a pass, never finished, pin {} MiB, past the relay's {} MiB room",
        pinned >> 20,
        ROOM >> 20
    );
}
