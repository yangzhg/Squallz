use std::{
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use squallz_core::lock_unpoisoned;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static TRACE_WRITE_LOCK: Mutex<()> = Mutex::new(());

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
    if let Err(e) = append_line(Path::new(&path), &line) {
        log::warn!("validation trace: write failed: {e}");
    }
}

fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;

    let _guard = lock_unpoisoned(&TRACE_WRITE_LOCK);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::append_line;
    use std::sync::{Arc, Barrier};

    #[test]
    fn concurrent_trace_lines_remain_independent_json_records() {
        let path = std::env::temp_dir().join(format!(
            "squallz-validation-trace-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let writers = 8;
        let lines_per_writer = 20;
        let start = Arc::new(Barrier::new(writers));
        let handles = (0..writers)
            .map(|writer| {
                let path = path.clone();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    let padding = "x".repeat(8 * 1024);
                    for sequence in 0..lines_per_writer {
                        let line = serde_json::json!({
                            "writer": writer,
                            "sequence": sequence,
                            "padding": padding,
                        })
                        .to_string();
                        append_line(&path, &line).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap();
        }

        let contents = std::fs::read_to_string(&path).unwrap();
        let records = contents
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(records.len(), writers * lines_per_writer);
        std::fs::remove_file(path).unwrap();
    }
}
