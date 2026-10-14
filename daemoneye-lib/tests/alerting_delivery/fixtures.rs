//! Shared rig for the delivery tests: a scripted sink, a temp store and a started manager.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::used_underscore_binding,
    clippy::clone_on_ref_ptr
)]

use async_trait::async_trait;
use daemoneye_lib::alerting::{
    AlertManager, AlertPublisher, AlertSink, AlertingError, DeliveryError, DeliveryResult,
    NoopPublisher, PublishError,
};
use daemoneye_lib::config::AlertingConfig;
use daemoneye_lib::models::{
    Alert, AlertSeverity, Completeness, DeliveryRecord, ProcessRecord, Uuid,
};
use daemoneye_lib::storage::EventStore;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tempfile::TempDir;
use tokio::sync::watch;
use tokio::time::Instant;

/// What a scripted sink does with one send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Ok,
    Transient,
    Permanent,
    Misconfigured,
    Hang,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: Uuid,
    pub at: Instant,
    pub marked_unpersisted: bool,
}

struct FakeState {
    script: VecDeque<Act>,
    default: Act,
    healthy: bool,
    log: Vec<Entry>,
}

/// A sink whose outcomes the test scripts and whose attempts it can read back.
pub struct Fake {
    name: String,
    local: bool,
    state: Mutex<FakeState>,
}

impl Fake {
    pub fn new(name: &str, local: bool, default: Act) -> Arc<Self> {
        Arc::new(Self {
            name: name.to_owned(),
            local,
            state: Mutex::new(FakeState {
                script: VecDeque::new(),
                default,
                healthy: true,
                log: Vec::new(),
            }),
        })
    }

    pub fn script(&self, acts: &[Act]) {
        self.state
            .lock()
            .unwrap()
            .script
            .extend(acts.iter().copied());
    }

    pub fn set_default(&self, act: Act) {
        self.state.lock().unwrap().default = act;
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.state.lock().unwrap().healthy = healthy;
    }

    pub fn log(&self) -> Vec<Entry> {
        self.state.lock().unwrap().log.clone()
    }

    pub fn attempts(&self) -> usize {
        self.state.lock().unwrap().log.len()
    }

    pub fn boxed(self: &Arc<Self>) -> Box<dyn AlertSink> {
        Box::new(Handle(Arc::clone(self)))
    }
}

struct Handle(Arc<Fake>);

#[async_trait]
impl AlertSink for Handle {
    async fn send(&self, alert: &Alert) -> Result<DeliveryResult, AlertingError> {
        let act = {
            let mut state = self.0.state.lock().unwrap();
            state.log.push(Entry {
                id: alert.id,
                at: Instant::now(),
                marked_unpersisted: alert.context.data.contains_key("not_persisted"),
            });
            let default = state.default;
            state.script.pop_front().unwrap_or(default)
        };
        match act {
            Act::Ok => Ok(DeliveryResult {
                sink_name: self.0.name.clone(),
                delivered_at: chrono::Utc::now(),
                duration_ms: 0,
            }),
            Act::Transient => Err(DeliveryError::from_http_status(503, None).into()),
            Act::Permanent => Err(DeliveryError::from_http_status(404, None).into()),
            Act::Misconfigured => Err(DeliveryError::from_http_status(401, None).into()),
            Act::Hang => std::future::pending().await,
        }
    }

    fn name(&self) -> &str {
        &self.0.name
    }

    async fn health_check(&self) -> Result<(), AlertingError> {
        if self.0.state.lock().unwrap().healthy {
            return Ok(());
        }
        Err(DeliveryError::from_http_status(503, None).into())
    }

    fn is_local(&self) -> bool {
        self.0.local
    }
}

pub fn alert(severity: AlertSeverity, title: &str) -> Alert {
    Alert::new(
        severity,
        title,
        "d",
        "rule-1",
        ProcessRecord::new(1, "p".to_owned()),
        Completeness::complete(),
    )
}

pub fn alert_keyed(severity: AlertSeverity, title: &str, key: &str) -> Alert {
    let mut made = alert(severity, title);
    key.clone_into(&mut made.deduplication_key);
    made
}

/// Alerting config with delivery bounds the tests care about; the rest keep their defaults.
pub fn config(tweak: impl FnOnce(&mut AlertingConfig)) -> AlertingConfig {
    let mut config = AlertingConfig::default();
    config.delivery.breaker_failure_threshold = 1_000;
    tweak(&mut config);
    config
}

pub fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap()
}

pub const SEED: u64 = 11;

pub struct Rig {
    pub _dir: Option<TempDir>,
    pub store: Arc<EventStore>,
    pub manager: AlertManager,
    pub cancel: watch::Sender<bool>,
}

pub fn temp_store() -> (TempDir, Arc<EventStore>) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(EventStore::new(dir.path().join("events.redb")).unwrap());
    (dir, store)
}

/// A started manager over a fresh temp store.
pub async fn rig(sinks: Vec<Box<dyn AlertSink>>, alerting: AlertingConfig) -> Rig {
    let (dir, store) = temp_store();
    let mut rig = rig_over(store, sinks, alerting).await;
    rig._dir = Some(dir);
    rig
}

/// A started manager over an existing store.
pub async fn rig_over(
    store: Arc<EventStore>,
    sinks: Vec<Box<dyn AlertSink>>,
    alerting: AlertingConfig,
) -> Rig {
    rig_publishing(store, sinks, alerting, Arc::new(NoopPublisher)).await
}

/// Counts publishes and optionally fails them.
pub struct CountingPublisher {
    pub published: AtomicUsize,
    pub fail: bool,
}

impl CountingPublisher {
    pub fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            published: AtomicUsize::new(0),
            fail,
        })
    }

    pub fn count(&self) -> usize {
        self.published.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AlertPublisher for CountingPublisher {
    async fn publish(&self, _alert: &Alert) -> Result<(), PublishError> {
        self.published.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(PublishError("bus down".to_owned()))
        } else {
            Ok(())
        }
    }
}

/// Publishes never complete, like a saturated broker.
pub struct StuckPublisher;

#[async_trait]
impl AlertPublisher for StuckPublisher {
    async fn publish(&self, _alert: &Alert) -> Result<(), PublishError> {
        std::future::pending().await
    }
}

pub async fn rig_publishing(
    store: Arc<EventStore>,
    sinks: Vec<Box<dyn AlertSink>>,
    alerting: AlertingConfig,
    publisher: Arc<dyn AlertPublisher>,
) -> Rig {
    let (cancel, cancel_rx) = watch::channel(false);
    let manager = AlertManager::start_with_rng_seed(
        Arc::clone(&store),
        sinks,
        &alerting,
        publisher,
        cancel_rx,
        SEED,
    )
    .await
    .unwrap();
    Rig {
        _dir: None,
        store,
        manager,
        cancel,
    }
}

pub fn row(store: &EventStore, sink: &str, id: Uuid) -> DeliveryRecord {
    store.get_delivery(sink, id).unwrap().unwrap()
}

/// Let every ready task and in-flight store call finish; moves the clock by one millisecond.
pub async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
}

pub async fn run_for(seconds: u64) {
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
}
