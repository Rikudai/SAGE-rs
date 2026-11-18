//! User confirmations: emit `ConfirmRequest`, wait for `Command::Confirm`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sage_core::events::{EventBus, SageEvent};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::lock;

type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>;

/// Pending yes/no questions to the user, keyed by request id.
#[derive(Clone)]
pub struct Confirmer {
    bus: EventBus,
    pending: Pending,
    timeout: Duration,
}

impl Confirmer {
    pub fn new(bus: EventBus, timeout: Duration) -> Self {
        Self { bus, pending: Arc::default(), timeout }
    }

    /// Asks the user; `false` on "no", timeout or cancellation.
    pub async fn ask(&self, question: &str, cancel: &CancellationToken) -> bool {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        lock(&self.pending).insert(id.clone(), tx);
        self.bus.emit(SageEvent::ConfirmRequest { id: id.clone(), question: question.to_string() });
        let yes = tokio::select! {
            _ = cancel.cancelled() => false,
            r = tokio::time::timeout(self.timeout, rx) => matches!(r, Ok(Ok(true))),
        };
        lock(&self.pending).remove(&id);
        yes
    }

    /// Delivers an answer; `false` when no question with that id is waiting.
    pub fn answer(&self, id: &str, yes: bool) -> bool {
        let tx = lock(&self.pending).remove(id);
        match tx {
            Some(tx) => tx.send(yes).is_ok(),
            None => false,
        }
    }

    /// Number of questions waiting for an answer.
    pub fn waiting(&self) -> usize {
        lock(&self.pending).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn answer_reaches_the_waiting_question() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let c = Confirmer::new(bus, Duration::from_secs(5));
        let c2 = c.clone();
        let cancel = CancellationToken::new();
        let task = tokio::spawn(async move { c2.ask("Desligar?", &cancel).await });
        let id = loop {
            if let Ok(SageEvent::ConfirmRequest { id, .. }) = rx.recv().await {
                break id;
            }
        };
        assert!(c.answer(&id, true));
        assert!(task.await.unwrap());
        assert_eq!(c.waiting(), 0);
        assert!(!c.answer("inexistente", true));
    }

    #[tokio::test]
    async fn cancellation_and_timeout_mean_no() {
        let c = Confirmer::new(EventBus::new(), Duration::from_millis(30));
        let cancel = CancellationToken::new();
        assert!(!c.ask("Pode?", &cancel).await);
        cancel.cancel();
        assert!(!c.ask("Pode?", &cancel).await);
    }
}
