use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use sha2::{Digest, Sha256};

use super::*;

const HOST: &str = "k3x9q2.p.bipagents.com";

fn quick() -> Pace {
    Pace {
        check_every: Duration::from_secs(3600),
        pending_min: Duration::ZERO,
        pending_max: Duration::ZERO,
        backoff_min: Duration::ZERO,
        backoff_max: Duration::ZERO,
    }
}

/// The door's self-signed certificate (bridge-lan.*), as lan.rs makes it.
fn door_files(dir: &Path) -> (String, String) {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap().self_signed(&key).unwrap();
    let (c, k) = (dir.join("bridge-lan.pem"), dir.join("bridge-lan.key"));
    std::fs::write(&c, cert.pem()).unwrap();
    std::fs::write(&k, key.serialize_pem()).unwrap();
    (c.to_string_lossy().into_owned(), k.to_string_lossy().into_owned())
}

/// A chain « from Let's Encrypt » for `host` and `key`: a leaf signed by a throwaway CA, then that CA.
fn chain_for(host: &str, key: &rcgen::KeyPair) -> String {
    let ca_key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = ca.self_signed(&ca_key).unwrap();
    let leaf = rcgen::CertificateParams::new(vec![host.to_string()]).unwrap().signed_by(key, &ca, &ca_key).unwrap();
    format!("{}{}", leaf.pem(), ca.pem())
}

/// A place certificate written as [`store`] does.
fn place_files(dir: &Path, host: &str, expires: i64) -> String {
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let chain = chain_for(host, &key);
    std::fs::write(dir.join(CERT_NAME), &chain).unwrap();
    std::fs::write(dir.join(KEY_NAME), key.serialize_pem()).unwrap();
    std::fs::write(dir.join(META_NAME), json!({"host": host, "expires": expires}).to_string()).unwrap();
    chain
}

fn leaf_print(chain_pem: &str) -> String {
    let leaf = pem::parse_many(chain_pem).unwrap().into_iter().next().unwrap();
    hex::encode(Sha256::digest(leaf.contents()))
}

/// The door serving `door` on 127.0.0.1 (a router that answers « ok »).
async fn serve(door: Arc<DoorCerts>) -> (u16, CancellationToken) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let stop = CancellationToken::new();
    let router = Router::new().route("/", get(|| async { "ok" }));
    crate::main_cli::serve_door_on(listener, router, crate::main_cli::door_acceptor(door).unwrap(), stop.clone());
    (port, stop)
}

/// The leaf certificate's SHA-256 a client gets asking for `name` (an IP address: no SNI at all).
async fn served(port: u16, name: &str) -> String {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .tls_info(true)
        .resolve(name, std::net::SocketAddr::from(([127, 0, 0, 1], port)))
        .build()
        .unwrap();
    let resp = client.get(format!("https://{name}:{port}/")).send().await.unwrap();
    let info = resp.extensions().get::<reqwest::tls::TlsInfo>().unwrap();
    hex::encode(Sha256::digest(info.peer_certificate().unwrap()))
}

#[test]
fn renewal_when_none_or_under_30_days() {
    let now = 1_800_000_000;
    let held = |days: i64| Held { host: HOST.into(), expires: now + days * 86_400 };
    assert!(needs_renewal(None, now));
    assert!(needs_renewal(Some(&held(29)), now));
    assert!(needs_renewal(Some(&held(-1)), now));
    assert!(!needs_renewal(Some(&held(31)), now));
    assert!(!needs_renewal(Some(&held(89)), now));
}

#[test]
fn backoff_doubles_from_a_minute_to_six_hours_and_respects_the_guichet() {
    let pace = Pace::default();
    let s = Duration::from_secs;
    assert_eq!(backoff(&pace, 1, Duration::ZERO), s(60));
    assert_eq!(backoff(&pace, 2, Duration::ZERO), s(120));
    assert_eq!(backoff(&pace, 4, Duration::ZERO), s(480));
    assert_eq!(backoff(&pace, 1, s(10)), s(60)); // never under a minute
    assert_eq!(backoff(&pace, 1, s(900)), s(900)); // the guichet asks for more
    assert_eq!(backoff(&pace, 40, Duration::ZERO), s(6 * 3600));
    assert_eq!(backoff(&pace, 1, s(86_400)), s(6 * 3600));
}

#[test]
fn the_request_names_exactly_the_host() {
    let (key, csr) = new_request(HOST).unwrap();
    assert!(key.contains("BEGIN PRIVATE KEY"));
    let der = pem::parse(&csr).unwrap();
    assert_eq!(der.tag(), "CERTIFICATE REQUEST");
    let bytes = der.contents();
    let count = |needle: &[u8]| bytes.windows(needle.len()).filter(|w| *w == needle).count();
    assert_eq!(count(HOST.as_bytes()), 2, "once as the common name, once as the DNS name");
    assert_eq!(count(&[0x06, 0x03, 0x55, 0x04, 0x03]), 1, "commonName");
    assert_eq!(count(&[0x06, 0x03, 0x55, 0x1d, 0x11]), 1, "subjectAltName");
    // the CSR's key is the key kept here
    let pair = rcgen::KeyPair::from_pem(&key).unwrap();
    assert_eq!(count(pair.public_key_raw()), 1);
}

#[tokio::test]
async fn the_host_gets_the_real_certificate_anything_else_the_door_one() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let door_print = leaf_print(&std::fs::read_to_string(&cert).unwrap());
    let chain = place_files(dir.path(), HOST, 1_900_000_000);
    let door = DoorCerts::load(&cert, &key).unwrap();
    assert_eq!(door.serving(), Some(Held { host: HOST.into(), expires: 1_900_000_000 }));
    let (port, stop) = serve(door).await;
    assert_eq!(served(port, HOST).await, leaf_print(&chain));
    assert_eq!(served(port, &HOST.to_uppercase()).await, leaf_print(&chain));
    assert_eq!(served(port, "other.p.bipagents.com").await, door_print);
    assert_eq!(served(port, "127.0.0.1").await, door_print); // an IP address: no SNI, what the apps pin
    stop.cancel();
}

#[tokio::test]
async fn a_new_certificate_is_served_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let door_print = leaf_print(&std::fs::read_to_string(&cert).unwrap());
    let door = DoorCerts::load(&cert, &key).unwrap();
    assert_eq!(door.serving(), None);
    let (port, stop) = serve(door.clone()).await;
    assert_eq!(served(port, HOST).await, door_print);

    let first = place_files(dir.path(), HOST, 1_900_000_000);
    assert!(door.reload().is_some());
    assert_eq!(served(port, HOST).await, leaf_print(&first));
    let renewed = place_files(dir.path(), HOST, 1_905_000_000);
    assert_eq!(door.reload().unwrap().expires, 1_905_000_000);
    assert_eq!(served(port, HOST).await, leaf_print(&renewed));

    // a key that isn't the certificate's: the door's own certificate only, never a broken handshake
    let other = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    std::fs::write(dir.path().join(KEY_NAME), other.serialize_pem()).unwrap();
    assert_eq!(door.reload(), None);
    assert_eq!(served(port, HOST).await, door_print);
    stop.cancel();
}

// -- a fake guichet ------------------------------------------------------------------------------------------------

/// What the fake guichet answers to `POST /v1/places/cert`, in order.
enum Answer {
    Status(u16, Value),
    /// 200 with a chain for the pending key (read from the place's folder: the fake has no CSR parser).
    Issue,
}

struct Fake {
    dir: PathBuf,
    host_status: u16,
    answers: Mutex<VecDeque<Answer>>,
    csrs: Mutex<Vec<String>>,
    host_calls: AtomicUsize,
}

async fn fake_host(State(fake): State<Arc<Fake>>, headers: axum::http::HeaderMap) -> impl IntoResponse {
    assert_eq!(headers.get("authorization").unwrap(), "Bearer place-token");
    fake.host_calls.fetch_add(1, Ordering::SeqCst);
    let status = StatusCode::from_u16(fake.host_status).unwrap();
    if fake.host_status == 200 {
        (status, Json(json!({"host": HOST})))
    } else {
        (status, Json(json!({"detail": "unknown place"})))
    }
}

async fn fake_cert(State(fake): State<Arc<Fake>>, Json(body): Json<Value>) -> impl IntoResponse {
    fake.csrs.lock().unwrap().push(body["csr"].as_str().unwrap().to_string());
    let answer = fake.answers.lock().unwrap().pop_front().expect("an answer left");
    match answer {
        Answer::Status(status, data) => (StatusCode::from_u16(status).unwrap(), Json(data)),
        Answer::Issue => {
            let key = std::fs::read_to_string(fake.dir.join(PENDING_KEY_NAME)).unwrap();
            let key = rcgen::KeyPair::from_pem(&key).unwrap();
            let chain = chain_for(HOST, &key);
            (StatusCode::OK, Json(json!({"host": HOST, "certificate": chain, "expires": 1_900_000_000})))
        }
    }
}

async fn guichet(dir: &Path, host_status: u16, answers: Vec<Answer>) -> (String, Arc<Fake>) {
    let fake = Arc::new(Fake {
        dir: dir.to_path_buf(),
        host_status,
        answers: Mutex::new(answers.into()),
        csrs: Mutex::new(Vec::new()),
        host_calls: AtomicUsize::new(0),
    });
    let router = Router::new()
        .route("/v1/places/host", get(fake_host))
        .route("/v1/places/cert", post(fake_cert))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await });
    (base, fake)
}

#[tokio::test]
async fn pending_then_issued_with_the_same_request_even_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let answers = vec![
        Answer::Status(202, json!({"retry_after": 20})),
        Answer::Status(202, json!({"retry_after": 20})),
        Answer::Issue,
    ];
    let (base, fake) = guichet(dir.path(), 200, answers).await;
    let http = reqwest::Client::new();
    let door = DoorCerts::load(&cert, &key).unwrap();
    assert_eq!(step(&http, &base, "place-token", &door).await, Step::Pending(Duration::from_secs(20)));
    use std::os::unix::fs::PermissionsExt;
    let mode = |name: &str| std::fs::metadata(dir.path().join(name)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(PENDING_KEY_NAME), 0o600);

    // the bridge restarts: the order on disk is asked again, no new key, no new host lookup
    let door = DoorCerts::load(&cert, &key).unwrap();
    assert_eq!(step(&http, &base, "place-token", &door).await, Step::Pending(Duration::from_secs(20)));
    let held = Held { host: HOST.into(), expires: 1_900_000_000 };
    assert_eq!(step(&http, &base, "place-token", &door).await, Step::Issued(held.clone()));

    let csrs = fake.csrs.lock().unwrap().clone();
    assert_eq!(csrs.len(), 3);
    assert!(csrs.iter().all(|c| *c == csrs[0]));
    assert_eq!(fake.host_calls.load(Ordering::SeqCst), 1);
    assert_eq!(door.serving(), Some(held.clone()));
    assert_eq!(super::held(dir.path()), Some(held));
    assert_eq!(mode(KEY_NAME), 0o600);
    assert!(!dir.path().join(PENDING_NAME).exists() && !dir.path().join(PENDING_KEY_NAME).exists());
    let (port, stop) = serve(door).await;
    assert_eq!(served(port, HOST).await, leaf_print(&std::fs::read_to_string(dir.path().join(CERT_NAME)).unwrap()));
    stop.cancel();
}

#[tokio::test]
async fn busy_guichet_backs_off_and_keeps_the_order() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let answers = vec![Answer::Status(429, json!({"detail": "Let's Encrypt rate limit", "retry_after": 900}))];
    let (base, _fake) = guichet(dir.path(), 200, answers).await;
    let door = DoorCerts::load(&cert, &key).unwrap();
    let got = step(&reqwest::Client::new(), &base, "place-token", &door).await;
    assert_eq!(got, Step::Failed(Duration::from_secs(900), "Let's Encrypt rate limit".into()));
    assert!(pending(dir.path()).is_some(), "the same CSR is asked again later");
    assert_eq!(backoff(&Pace::default(), 1, Duration::from_secs(900)), Duration::from_secs(900));
}

#[tokio::test]
async fn a_refused_request_starts_a_new_order_next_time() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let (base, _fake) = guichet(dir.path(), 200, vec![Answer::Status(400, json!({"detail": "wrong host"}))]).await;
    let door = DoorCerts::load(&cert, &key).unwrap();
    let got = step(&reqwest::Client::new(), &base, "place-token", &door).await;
    assert_eq!(got, Step::Failed(Duration::ZERO, "wrong host".into()));
    assert!(pending(dir.path()).is_none());
}

#[tokio::test]
async fn an_unknown_place_stops_asking() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let (base, fake) = guichet(dir.path(), 404, vec![]).await;
    let door = DoorCerts::load(&cert, &key).unwrap();
    assert_eq!(step(&reqwest::Client::new(), &base, "place-token", &door).await, Step::Unknown("unknown place".into()));
    // the loop gives up for good (until the bridge starts again)
    let finder = move || Some((base.clone(), "place-token".to_string()));
    let run = run_forever(reqwest::Client::new(), door.clone(), finder, quick(), CancellationToken::new());
    tokio::time::timeout(Duration::from_secs(10), run).await.expect("run_forever returns");
    assert_eq!(fake.host_calls.load(Ordering::SeqCst), 2);
    assert_eq!(door.serving(), None);
}

#[tokio::test]
async fn the_loop_gets_a_certificate_and_a_self_install_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(dir.path());
    let answers = vec![Answer::Status(503, json!({"detail": "busy"})), Answer::Status(202, json!({})), Answer::Issue];
    let (base, fake) = guichet(dir.path(), 200, answers).await;
    let door = DoorCerts::load(&cert, &key).unwrap();
    let stop = CancellationToken::new();
    let finder = move || Some((base.clone(), "place-token".to_string()));
    let run = tokio::spawn(run_forever(reqwest::Client::new(), door.clone(), finder, quick(), stop.clone()));
    for _ in 0..200 {
        if door.serving().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(door.serving().map(|h| h.host), Some(HOST.to_string()));
    assert_eq!(fake.csrs.lock().unwrap().len(), 3);
    stop.cancel();
    run.await.unwrap();

    // no guichet settings (a self-install): nothing made, nothing asked, it only waits and stops with the bridge
    let other = tempfile::tempdir().unwrap();
    let (cert, key) = door_files(other.path());
    let door = DoorCerts::load(&cert, &key).unwrap();
    let looked = Arc::new(AtomicUsize::new(0));
    let counter = looked.clone();
    let finder = move || {
        counter.fetch_add(1, Ordering::SeqCst);
        None
    };
    let stop = CancellationToken::new();
    let run = tokio::spawn(run_forever(reqwest::Client::new(), door.clone(), finder, quick(), stop.clone()));
    tokio::time::sleep(Duration::from_millis(100)).await;
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), run).await.expect("stops with the bridge").unwrap();
    assert_eq!(looked.load(Ordering::SeqCst), 1, "looked once, then waits the hour");
    assert!(pending(other.path()).is_none() && door.serving().is_none());
}
