use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Record};
use tracing::{Event, Id, Metadata, Subscriber};

#[derive(Clone, Debug, Default)]
pub(crate) struct CapturedEvent {
    fields: BTreeMap<String, String>,
}

impl CapturedEvent {
    pub(crate) fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }
}

#[derive(Clone, Default)]
struct CaptureSubscriber {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

pub(crate) fn capture_events<F, R>(f: F) -> (R, Vec<CapturedEvent>)
where
    F: FnOnce() -> R,
{
    static CAPTURE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = CAPTURE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let subscriber = CaptureSubscriber::default();
    let events = Arc::clone(&subscriber.events);
    let result = tracing::subscriber::with_default(subscriber, || {
        // Some tests exercise the same callsites before capture is installed.
        // Rebuild while our subscriber is the default so tracing's callsite
        // cache does not keep a stale "not interested" decision.
        tracing::callsite::rebuild_interest_cache();
        f()
    });
    tracing::callsite::rebuild_interest_cache();
    let captured = events.lock().unwrap().clone();
    (result, captured)
}

impl Subscriber for CaptureSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        self.events.lock().unwrap().push(CapturedEvent {
            fields: visitor.fields,
        });
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[derive(Default)]
struct FieldVisitor {
    fields: BTreeMap<String, String>,
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields
            .insert(field.name().to_string(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }
}
