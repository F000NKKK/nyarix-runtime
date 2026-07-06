//! Shutdown trigger sources (see issue #44's "Получение сигнала (Ctrl+C,
//! API, event)").
//!
//! Of the three trigger sources named in the issue:
//! - **Ctrl+C** — [`cancel_on_ctrl_c`], below.
//! - **API** — just call `.cancel()` on any [`CancellationToken`]
//!   yourself; there's nothing to build for this one.
//! - **event** — **not implemented**. Triggering shutdown from an
//!   [`crate::Event`] would need a specific event to mean "shut down now",
//!   and none of #48's 10 event variants represents that (they're all
//!   observations *about* the Runtime — module loaded, flow built,
//!   handshake completed, ... — not commands *to* it). Inventing an
//!   11th variant wasn't part of #48's spec, so this is left for whoever
//!   decides what should actually trigger a shutdown via the event bus
//!   (a `ShutdownRequested` event? a control-plane RPC that both
//!   publishes an event and cancels a token? unclear without a concrete
//!   use case).

use tokio_util::sync::CancellationToken;

/// Create a [`CancellationToken`] that cancels itself when the process
/// receives Ctrl+C (SIGINT).
///
/// # Panics
/// Panics if installing the Ctrl+C handler fails (see
/// [`tokio::signal::ctrl_c`]'s docs) — this can happen if, for example, a
/// conflicting signal handler is already installed elsewhere, which is a
/// genuine misconfiguration worth failing loudly on at startup rather
/// than silently never being able to shut down cleanly.
#[must_use]
pub fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let child = token.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        tracing::info!("received Ctrl+C, requesting shutdown");
        child.cancel();
    });
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn token_starts_uncancelled() {
        let token = cancel_on_ctrl_c();
        assert!(!token.is_cancelled());
    }

    #[tokio::test]
    async fn api_trigger_is_just_cancel() {
        // "API" trigger from the issue spec: no dedicated API needed,
        // callers just hold the token and cancel it themselves.
        let token = CancellationToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }
}
