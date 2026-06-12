use std::{
    sync::OnceLock,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();

fn unix_ms_since_epoch_or_zero(now: SystemTime) -> u128 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis(),
        Err(_) => 0,
    }
}

pub(crate) fn mark_process_start() {
    PROCESS_START.get_or_init(Instant::now);
    trace("process.start", json!({}));
}

pub(crate) fn trace(event: &str, payload: serde_json::Value) {
    let Ok(path) = std::env::var("SQUALLZ_VALIDATION_TRACE") else {
        return;
    };
    let process_ms = PROCESS_START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis();
    let unix_ms = unix_ms_since_epoch_or_zero(SystemTime::now());
    let line = json!({
        "event": event,
        "unix_ms": unix_ms,
        "process_ms": process_ms,
        "payload": payload,
    })
    .to_string();
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")
    };
    if let Err(e) = write() {
        log::warn!("validation trace: write failed: {e}");
    }
}
