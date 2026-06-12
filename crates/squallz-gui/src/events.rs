//! Event channel abstraction. Job logic emits through [`EventSink`] so it
//! can be exercised in unit tests without a Tauri window; `main.rs` plugs in
//! the real `AppHandle`.

use serde::Serialize;

/// Event names shared with the frontend.
pub const EV_PROGRESS: &str = "job://progress";
pub const EV_STATE: &str = "job://state";
pub const EV_ASK_CONFLICT: &str = "job://ask-conflict";
pub const EV_ASK_PASSWORD: &str = "job://ask-password";

/// Minimal emit interface (implemented by the Tauri `AppHandle` in main.rs
/// and by a buffering fake in tests).
pub trait EventSink: Send + Sync {
    /// Emits one event with a JSON payload.
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

/// Serializes and emits a typed payload.
pub fn emit<T: Serialize>(sink: &dyn EventSink, event: &str, payload: &T) {
    match serde_json::to_value(payload) {
        Ok(value) => sink.emit_json(event, value),
        Err(e) => log::error!("events: cannot serialize payload for {event}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::ser::Error as _;
    use serde_json::json;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl EventSink for RecordingSink {
        fn emit_json(&self, event: &str, payload: serde_json::Value) {
            let mut events = match self.events.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            events.push((event.to_string(), payload));
        }
    }

    impl RecordingSink {
        fn events(&self) -> Vec<(String, serde_json::Value)> {
            match self.events.lock() {
                Ok(guard) => guard.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            }
        }
    }

    struct FailingPayload;

    impl Serialize for FailingPayload {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(S::Error::custom("synthetic serialization failure"))
        }
    }

    #[test]
    fn event_names_keep_frontend_contract() {
        assert_eq!(EV_PROGRESS, "job://progress");
        assert_eq!(EV_STATE, "job://state");
        assert_eq!(EV_ASK_CONFLICT, "job://ask-conflict");
        assert_eq!(EV_ASK_PASSWORD, "job://ask-password");
    }

    #[test]
    fn emit_serializes_typed_payload_to_sink() {
        let sink = RecordingSink::default();

        emit(
            &sink,
            EV_STATE,
            &json!({ "id": "job-1", "state": "running" }),
        );

        assert_eq!(
            sink.events(),
            vec![(
                EV_STATE.to_string(),
                json!({ "id": "job-1", "state": "running" })
            )]
        );
    }

    #[test]
    fn emit_skips_unserializable_payload_without_calling_sink() {
        let sink = RecordingSink::default();

        emit(&sink, EV_PROGRESS, &FailingPayload);

        assert!(sink.events().is_empty());
    }
}
