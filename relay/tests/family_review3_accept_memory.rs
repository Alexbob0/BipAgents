//! Security review, third pass (d06356b5d): the lists of passes a mailbox accepts are kept for every mailbox that
//! ever said one (`POST /v1/family/passes`, up to 1,000 ids of 32 characters), and forgotten only with a mailbox that
//! polled and went (`sweep` looks at `seen` only). A mailbox that says a list and never polls keeps it in the relay's
//! memory until it restarts. Mailboxes are free (20 an hour per address, per /64 in IPv6: a /48 gives 65,536): about
//! 70 KB each, a million an hour from one IPv6 tunnel, the relay (and the push notifications) out of memory within
//! the hour. Fails on d06356b5d; should pass once a list is kept only for a mailbox polling (or a moment after), or
//! bounded in all.

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lists_of_mailboxes_that_never_poll_are_not_kept() {
    const MAILBOXES: usize = 100;
    let family = FamilyConfig { mailboxes_per_ip_hour: 10_000, ..FamilyConfig::default() };
    let relay = Arc::new(Relay::new(config(family), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    let (url, http) = (format!("http://{addr}"), reqwest::Client::new());
    let mut tokens = Vec::new();
    for _ in 0..MAILBOXES {
        let made: Value =
            http.post(format!("{url}/v1/family/mailbox")).json(&json!({})).send().await.unwrap().json().await.unwrap();
        tokens.push(made["token"].as_str().unwrap().to_string());
    }
    let ids: Vec<String> = (0..1000).map(|i| format!("{i:032}")).collect();
    let list = json!({"accept": ids});
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = LIVE.load(Ordering::Relaxed);
    for token in &tokens {
        let said = http.post(format!("{url}/v1/family/passes")).bearer_auth(token).json(&list).send().await.unwrap();
        assert_eq!(said.status(), 204);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let kept = LIVE.load(Ordering::Relaxed) - before;
    assert!(
        kept < (2 << 20) as isize,
        "{MAILBOXES} mailboxes that never poll keep {} MiB of lists in the relay",
        kept >> 20
    );
}
