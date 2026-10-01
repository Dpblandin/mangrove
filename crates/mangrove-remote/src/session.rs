use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use tokio::sync::watch;

use crate::{RemoteError, Result};

/// Cloneable cancellation signal usable by HTTP, SSH and mock transports.
#[derive(Clone, Debug)]
pub struct CancellationToken {
    signal: watch::Sender<bool>,
    parent: Option<Arc<CancellationToken>>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self {
            signal: watch::channel(false).0,
            parent: None,
        }
    }

    pub fn cancel(&self) {
        self.signal.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.signal.borrow()
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
    }

    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() {
            Err(RemoteError::Cancelled)
        } else {
            Ok(())
        }
    }

    pub fn child_token(&self) -> Self {
        Self {
            signal: watch::channel(false).0,
            parent: Some(Arc::new(self.clone())),
        }
    }

    pub(crate) fn cancelled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            let mut receiver = self.signal.subscribe();
            let own_signal = async {
                while !*receiver.borrow_and_update() {
                    if receiver.changed().await.is_err() {
                        return;
                    }
                }
            };
            if let Some(parent) = &self.parent {
                tokio::select! {
                    _ = own_signal => {},
                    _ = parent.cancelled() => {},
                }
            } else {
                own_signal.await;
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct SessionTicket {
    pub profile_id: String,
    pub account_id: String,
    pub epoch: u64,
    pub cancellation: CancellationToken,
}

const MAX_CACHE_ENTRIES: usize = 64;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

type CacheKey = (String, String);

struct SessionState {
    profile_id: String,
    epoch: u64,
    cancellation: CancellationToken,
    cache: HashMap<CacheKey, Arc<[u8]>>,
    order: VecDeque<CacheKey>,
    cache_bytes: usize,
}

impl Drop for SessionState {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone)]
pub struct ProfileSession {
    state: Arc<Mutex<SessionState>>,
}

impl ProfileSession {
    pub fn new(profile_id: impl Into<String>) -> Self {
        Self {
            state: Arc::new(Mutex::new(SessionState {
                profile_id: profile_id.into(),
                epoch: 0,
                cancellation: CancellationToken::new(),
                cache: HashMap::new(),
                order: VecDeque::new(),
                cache_bytes: 0,
            })),
        }
    }

    pub fn ticket(&self, account_id: impl Into<String>) -> SessionTicket {
        let state = self.state.lock().expect("profile session poisoned");
        ticket(&state, account_id.into())
    }

    /// Also used after account credentials/configuration change; same-profile
    /// switching deliberately renews the epoch, connections and memory cache.
    pub fn switch(&self, profile_id: impl Into<String>) -> SessionTicket {
        let mut state = self.state.lock().expect("profile session poisoned");
        state.cancellation.cancel();
        state.profile_id = profile_id.into();
        state.epoch = state.epoch.wrapping_add(1);
        state.cancellation = CancellationToken::new();
        state.cache.clear();
        state.order.clear();
        state.cache_bytes = 0;
        ticket(&state, String::new())
    }

    pub fn invalidate(&self) -> SessionTicket {
        let mut state = self.state.lock().expect("profile session poisoned");
        state.cancellation.cancel();
        state.epoch = state.epoch.wrapping_add(1);
        state.cancellation = CancellationToken::new();
        state.cache.clear();
        state.order.clear();
        state.cache_bytes = 0;
        ticket(&state, String::new())
    }

    pub fn is_current(&self, ticket: &SessionTicket) -> bool {
        matches(
            &self.state.lock().expect("profile session poisoned"),
            ticket,
        )
    }

    /// Cache keys are always scoped by the ticket's profile epoch and account.
    /// Store normalized, credential-free responses only. Each value and the
    /// complete cache are bounded; rejected late results cannot refill it.
    pub fn cache_put(
        &self,
        ticket: &SessionTicket,
        key: impl Into<String>,
        value: Vec<u8>,
    ) -> bool {
        let mut state = self.state.lock().expect("profile session poisoned");
        let key = key.into();
        if !matches(&state, ticket) || value.len() > MAX_CACHE_BYTES || key.len() > 2048 {
            return false;
        }
        let key = (ticket.account_id.clone(), key);
        if let Some(old) = state.cache.remove(&key) {
            state.cache_bytes -= old.len();
            state.order.retain(|entry| entry != &key);
        }
        while state.cache.len() >= MAX_CACHE_ENTRIES
            || state.cache_bytes.saturating_add(value.len()) > MAX_CACHE_BYTES
        {
            if let Some(old_key) = state.order.pop_front() {
                if let Some(old) = state.cache.remove(&old_key) {
                    state.cache_bytes -= old.len();
                }
            } else {
                break;
            }
        }
        state.cache_bytes += value.len();
        state.cache.insert(key.clone(), Arc::from(value));
        state.order.push_back(key);
        true
    }

    pub fn cache_get(&self, ticket: &SessionTicket, key: &str) -> Option<Arc<[u8]>> {
        let state = self.state.lock().expect("profile session poisoned");
        if !matches(&state, ticket) {
            return None;
        }
        state
            .cache
            .get(&(ticket.account_id.clone(), key.to_owned()))
            .cloned()
    }
}

fn ticket(state: &SessionState, account_id: String) -> SessionTicket {
    SessionTicket {
        profile_id: state.profile_id.clone(),
        account_id,
        epoch: state.epoch,
        cancellation: state.cancellation.child_token(),
    }
}

fn matches(state: &SessionState, ticket: &SessionTicket) -> bool {
    ticket.profile_id == state.profile_id
        && ticket.epoch == state.epoch
        && !ticket.cancellation.is_cancelled()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::mpsc,
        thread,
        time::{Duration, Instant},
    };

    #[test]
    fn profile_switch_aborts_old_tasks_and_rejects_late_results() {
        let session = ProfileSession::new("personal");
        let old = session.ticket("account");
        let (started_tx, started_rx) = mpsc::channel();
        let (reply_tx, reply_rx) = mpsc::channel();
        let worker_session = session.clone();
        let worker_ticket = old.clone();
        let worker = thread::spawn(move || {
            started_tx.send(()).unwrap();
            // A mock deliberately simulates a non-cooperative late transport.
            reply_rx.recv().unwrap();
            worker_session.cache_put(
                &worker_ticket,
                "repositories",
                b"private-old-profile".to_vec(),
            )
        });
        started_rx.recv().unwrap();
        session.switch("professional");
        assert!(old.cancellation.is_cancelled());
        reply_tx.send(()).unwrap();
        assert!(!worker.join().unwrap());
        let new = session.ticket("account");
        assert!(session.is_current(&new));
        assert!(session.cache_get(&new, "repositories").is_none());
        session.switch("personal");
        assert!(!session.is_current(&old));
    }

    #[test]
    fn caches_and_cancellation_are_separate_for_accounts() {
        let session = ProfileSession::new("personal");
        let a = session.ticket("first-account");
        let b = session.ticket("second-account");
        assert!(session.cache_put(&a, "repositories", b"first".to_vec()));
        assert!(session.cache_get(&b, "repositories").is_none());
        assert!(session.cache_put(&b, "repositories", b"second".to_vec()));
        assert_eq!(&*session.cache_get(&a, "repositories").unwrap(), b"first");
        a.cancellation.cancel();
        assert!(!session.is_current(&a));
        assert!(session.is_current(&b));
        assert_eq!(&*session.cache_get(&b, "repositories").unwrap(), b"second");
        session.invalidate();
        assert!(b.cancellation.is_cancelled());
        assert!(session
            .cache_get(&session.ticket("second-account"), "repositories")
            .is_none());
    }

    #[test]
    fn memory_cache_bounds_entries_and_bytes() {
        let session = ProfileSession::new("profile");
        let ticket = session.ticket("account");
        for id in 0..MAX_CACHE_ENTRIES + 1 {
            assert!(session.cache_put(&ticket, id.to_string(), vec![1]));
        }
        assert!(session.cache_get(&ticket, "0").is_none());
        assert!(session.cache_get(&ticket, "64").is_some());
        assert!(session.cache_put(&ticket, "large", vec![1; MAX_CACHE_BYTES]));
        assert!(session.cache_get(&ticket, "64").is_none());
        assert!(session.cache_put(&ticket, "new", vec![2]));
        assert!(session.cache_get(&ticket, "large").is_none());
        assert!(!session.cache_put(&ticket, "oversized", vec![0; MAX_CACHE_BYTES + 1]));
    }

    #[test]
    fn cancellation_wakes_an_in_flight_future_without_waiting_for_timeout() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let cancellation = parent.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            cancellation.cancel();
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = Instant::now();
        runtime.block_on(async {
            tokio::select! {
                _ = child.cancelled() => {},
                _ = tokio::time::sleep(Duration::from_secs(2)) => panic!("cancellation did not wake the request"),
            }
        });
        worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn dropping_the_last_session_cancels_its_tickets() {
        let session = ProfileSession::new("profile");
        let ticket = session.ticket("account");
        drop(session);
        assert!(ticket.cancellation.is_cancelled());
    }
}
