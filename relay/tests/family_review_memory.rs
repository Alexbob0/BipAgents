//! Security review of the family rendezvous (family.rs, branch family-relay-anywhere): the memory one caller pins.
//!
//! `call` reads a request's whole body into memory (`to_bytes`, up to `request_bytes`, 16 MiB) before anything
//! bounds the bytes held: the 256 MiB cap (`queued_bytes`) and the 64-call queue only count calls already read, and
//! nothing times a body out. One mailbox (3,000 calls an hour) calling its own second mailbox (which polls) opens
//! N calls, sends each body but its last byte, and stops: N × 16 MiB stay in the relay's memory. 300 such calls are
//! 4.8 GB: the relay (and with it every push notification, same process) is killed for out of memory, again at will.
//!
//! This test counts the bytes allocated in the process (its own allocator): it fails on dd94437a8 and should pass
//! once the bytes being read are bounded with the bytes waiting (or bodies streamed to the poll instead of held).

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
        trust_forwarded_for: false,
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_bytes_of_calls_still_coming_in_are_capped_with_the_bytes_waiting() {
    const BODY: usize = 4 << 20; // a call's request may weigh this much here (16 MiB by default)
    const CAP: usize = 1 << 20; // the bytes waiting at the relay, every mailbox together (256 MiB by default)
    const CALLS: usize = 16;
    let family = FamilyConfig {
        request_bytes: BODY,
        queued_bytes: CAP,
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
    // the attacker's two mailboxes: one calls, the other (its own) polls, so that the calls are let in
    let ((_, caller), (target, target_token)) = (mailbox(&http, &url).await, mailbox(&http, &url).await);
    let poll = {
        let (http, url) = (http.clone(), url.clone());
        tokio::spawn(async move { http.get(format!("{url}/v1/family/next")).bearer_auth(target_token).send().await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = LIVE.load(Ordering::Relaxed);

    // CALLS calls, each body sent but its last byte, then nothing more (no timeout ever ends them)
    let mut held = Vec::new();
    for _ in 0..CALLS {
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        let head = format!(
            "POST /v1/family/call/{target} HTTP/1.1\r\nHost: relay\r\nAuthorization: Bearer {caller}\r\n\
             Content-Type: application/octet-stream\r\nContent-Length: {BODY}\r\n\r\n"
        );
        socket.write_all(head.as_bytes()).await.unwrap();
        let mut left = BODY - 1;
        while left > 0 {
            let n = left.min(ZEROS.len());
            socket.write_all(&ZEROS[..n]).await.unwrap();
            left -= n;
        }
        socket.flush().await.unwrap();
        held.push(socket);
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let pinned = LIVE.load(Ordering::Relaxed) - before;
    drop(held);
    poll.abort();
    assert!(
        pinned < (CAP + (4 << 20)) as isize,
        "{CALLS} calls never finished pin {} MiB in the relay, past its {} MiB cap on the bytes it holds",
        pinned >> 20,
        CAP >> 20
    );
}
