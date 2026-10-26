//! BipAgents push relay.
//!
//! People's bridges cannot hold the publisher's Apple key or Firebase account; the relay does, and forwards their
//! notifications. It stores nothing (relay tokens carry the device token, sealed), forwards no content (previews
//! are replaced by the generic text), and needs nothing but its own keys: one small binary, as many copies as wanted.
//!
//! - `POST /v1/register`   `{"platform": "apns"|"fcm", "token": "…", "environment": "sandbox"|"production"}`
//!   → `{"relay_token": "r1.…"}` (the app gives it to its bridge instead of its device token)
//! - `POST /v1/relay/push` `{"token": "r1.…", "payload": {…}, "push_type": "alert"|"background"|"liveactivity", "priority": 5|10,
//!   "collapse_id": "…"}` → `{"status": 200, "reason": null, "apns_id": "…"}`, Apple's answer; `status` 410 means
//!   the token is dead (the bridge forgets the device). Same shape as the bridges' own relay.
//! - `GET /health`

pub mod apns;
pub mod config;
pub mod fcm;
pub mod limits;
pub mod payload;
pub mod routes;
pub mod seal;

/// Apple's or Google's answer, as the bridge reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: u16,
    pub reason: Option<String>,
    pub id: Option<String>,
}
