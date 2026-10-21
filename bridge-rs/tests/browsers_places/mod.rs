//! Support for the browsers-places tests: a router of the module's own routes (with what Python's tests monkeypatch
//! given as hooks) around the state of a real app, driven without a socket like `TestApp`.
#![allow(dead_code)]

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request};
use axum::middleware;
use axum::Router;
use bipbridge::app::AppState;
use tower::ServiceExt;

use crate::common::TestResponse;

/// `routes` under /v1 (the `authed` ones behind the bridge key), with the app's state.
pub fn v1(state: &AppState, authed: Router<AppState>, open: Router<AppState>) -> Router {
    let auth = middleware::from_fn_with_state(state.clone(), bipbridge::auth::require_bridge_key);
    Router::new().nest("/v1", authed.layer(auth).merge(open)).with_state(state.clone())
}

pub async fn send(
    router: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Vec<u8>>,
) -> TestResponse {
    let mut builder = Request::builder().method(Method::from_bytes(method.as_bytes()).unwrap()).uri(path);
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let mut request = builder.body(body.map(Body::from).unwrap_or_else(Body::empty)).unwrap();
    request.extensions_mut().insert(ConnectInfo::<SocketAddr>("198.51.100.7:50000".parse().unwrap()));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec();
    TestResponse { status, headers, body }
}

pub async fn send_json(
    router: &Router,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
) -> TestResponse {
    let mut all = headers.to_vec();
    all.push(("content-type", "application/json"));
    send(router, method, path, &all, Some(serde_json::to_vec(body).unwrap())).await
}

/// HMAC-SHA256 over the parts, one per line (the apps' and the installer's proofs).
pub fn mac(key: &[u8], parts: &[&str]) -> String {
    bipbridge::browsers::pyx::hmac_hex(key, parts)
}
