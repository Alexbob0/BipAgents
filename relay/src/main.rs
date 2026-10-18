use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bipagents_relay::config::Config;
use bipagents_relay::routes::{router, Relay};
use bipagents_relay::seal::Sealer;

async fn shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut signal) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("keygen") {
        println!("{}", Sealer::generate_key());
        return;
    }
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("bipagents-relay: {error}");
            std::process::exit(2);
        }
    };
    let listen = config.listen;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(300))
        .build()
        .expect("HTTP client");
    let relay = match Relay::new(config, http) {
        Ok(relay) => Arc::new(relay),
        Err(error) => {
            eprintln!("bipagents-relay: {error}");
            std::process::exit(2);
        }
    };
    tracing::info!(%listen, apns = relay.apns.is_some(), fcm = relay.fcm.is_some(), "relay listening");
    let listener = tokio::net::TcpListener::bind(listen).await.expect("listen");
    axum::serve(listener, router(relay).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server");
}
