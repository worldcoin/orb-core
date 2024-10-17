//! Fetch a token for authorization on the backend. Requires
//! orb-short-lived-token-daemon running on the orb. If the daemon is not
//! present falls back to using static token.

use crate::{dbus::AuthTokenProxy, dd_incr, identification::ORB_TOKEN};
use eyre::Result;
use std::time::Duration;

const TOKEN_MONITOR_INTERVAL: Duration = Duration::from_secs(60);

/// Attempts to update `ORB_TOKEN` by communicating with the AuthToken daemon.
///
/// Note: This can hang indefinitely if the remote deadlocks or panics.
/// Consider wrapping it in a timeout.
async fn request_orb_token() -> Result<()> {
    let connection = Box::pin(zbus::Connection::session()).await?;
    let proxy = AuthTokenProxy::new(&connection).await?;

    proxy
        .token()
        .await
        .map(|new_token| {
            tracing::trace!("Got short lived token");
            // When poisoned, we discard the error with `into_inner`.
            // This is because we are about to overwrite the poisoned value anyway.
            let mut guard = ORB_TOKEN.write().unwrap_or_else(std::sync::PoisonError::into_inner);
            *guard = Ok(new_token);
        })
        .map_err(|e| eyre::eyre!("AuthToken daemon failed (maybe a D-Bus error): {e}"))
}

/// Initialize the orb token and then monitor for updates. When the token is
/// updated in the daemon, update the internal token value. if no token is
/// available, `ORB_TOKEN` will be an empty string.
pub async fn monitor_token() -> ! {
    loop {
        if let Err(e) = request_orb_token().await {
            tracing::warn!("Token monitor could not communicate with provider: {e}");
        }
        tokio::time::sleep(TOKEN_MONITOR_INTERVAL).await;
    }
}

/// Waits for `ORB_TOKEN` from the token daemon before returning.
pub async fn wait_for_token() {
    const TIME_BETWEEN_ATTEMPTS: Duration = Duration::from_secs(30);
    loop {
        let attempt_timeout = tokio::time::Instant::now() + TIME_BETWEEN_ATTEMPTS;
        let result = tokio::time::timeout_at(attempt_timeout, request_orb_token()).await;

        match result {
            Ok(Ok(())) => break, // We got the token successfully
            Ok(Err(e)) => {
                tracing::warn!("RPC to token daemon errored out, will retry shortly: {e}");
                dd_incr!("main.count.global.token_request_reattempts");
                // We sleep to avoid spamming attempts, such as when the daemon is not running.
                tokio::time::sleep_until(attempt_timeout).await;
            }
            Err(tokio::time::error::Elapsed { .. }) => {
                tracing::warn!("RPC to token daemon timed out, will retry now.");
                dd_incr!("main.count.global.token_request_reattempts");
            }
        }
    }
}
