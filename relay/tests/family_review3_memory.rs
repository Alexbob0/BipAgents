//! Security review, third pass (d06356b5d): the family routes that take a small JSON body read it whole, with no
//! limit. `routes()` puts `DefaultBodyLimit::disable()` on every family route (for `call` and `answer`, which stream
//! with their own limits), and `mailbox`, `pass` and `passes` take their body as `Bytes`, read in full before the
//! handler runs: `POST /v1/family/mailbox` — no token, the per-address quota checked only after — with a body that
//! never ends keeps it all in the relay's memory. One connection streaming zeros (`curl -T /dev/zero`) takes the
//! relay, and every push notification with it, out of memory in seconds. Present since dd94437a8.
//! Fails on d06356b5d; should pass once these routes have a small body limit (64 KiB is plenty).

use std::alloc::{GlobalAlloc, Layout, System};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bipagents_relay::config::Config;
use bipagents_relay::family::FamilyConfig;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mailbox_request_that_never_ends_holds_next_to_nothing() {
    const SENT: usize = 64 << 20;
    let relay = Arc::new(Relay::new(config(FamilyConfig::default()), reqwest::Client::new()).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let before = LIVE.load(Ordering::Relaxed);
    // no token: anyone may ask for a mailbox; a chunked body, never ended
    let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
    let head = "POST /v1/family/mailbox HTTP/1.1\r\nHost: relay\r\nContent-Type: application/json\r\n\
                Transfer-Encoding: chunked\r\n\r\n";
    socket.write_all(head.as_bytes()).await.unwrap();
    let mut sent = 0;
    while sent < SENT {
        let size = format!("{:x}\r\n", ZEROS.len());
        let wrote = async {
            socket.write_all(size.as_bytes()).await?;
            socket.write_all(&ZEROS).await?;
            socket.write_all(b"\r\n").await
        };
        if wrote.await.is_err() {
            break; // the relay stopped reading and closed: nothing kept
        }
        sent += ZEROS.len();
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let pinned = LIVE.load(Ordering::Relaxed) - before;
    drop(socket);
    assert!(
        pinned < (4 << 20) as isize,
        "a request for a mailbox, {} MiB of body and not ended, holds {} MiB in the relay",
        sent >> 20,
        pinned >> 20
    );
}
