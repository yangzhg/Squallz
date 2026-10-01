mod common;

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryPath, ExtractOptions, FormatError, NoProgress, OpenOptions,
    ProgressPhase, ProgressSink,
};

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Phase(ProgressPhase),
    Bytes(u64, u64, String),
}

#[derive(Default)]
struct OpeningProgress {
    events: Mutex<Vec<Event>>,
    cancel: Option<ControlToken>,
}

impl ProgressSink for OpeningProgress {
    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.events.lock().unwrap().push(Event::Phase(phase));
        if phase == ProgressPhase::ArchiveOpen {
            assert!(interruptible);
            if let Some(control) = &self.cancel {
                control.cancel();
            }
        }
    }

    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        self.events
            .lock()
            .unwrap()
            .push(Event::Bytes(done, total, current.display.clone()));
    }
}

fn run_operation(
    operation: &str,
    archive: &Path,
    destination: &Path,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<(), FormatError> {
    let engine = engine();
    let open = OpenOptions::default();
    let extract = ExtractOptions::default();
    match operation {
        "test" => engine
            .test_summary(
                archive,
                &open,
                &squallz_format_api::SafetyLimits::default(),
                progress,
                control,
            )
            .map(|report| assert!(report.is_ok())),
        "extract" => engine.extract(
            archive,
            destination,
            None,
            &open,
            &extract,
            progress,
            control,
        ),
        "extract_report" => engine
            .extract_with_report(
                archive,
                destination,
                None,
                &open,
                &extract,
                progress,
                control,
            )
            .map(drop),
        "guarded_extract" => engine
            .plan_and_extract_with_report_guarded_and_structure_controlled(
                archive,
                destination,
                archive,
                false,
                &open,
                &extract,
                progress,
                control,
                None,
                |_, _| Ok(None),
                |_| Ok(()),
            )
            .map(drop),
        "convert" => engine.convert(
            archive,
            destination,
            &open,
            &CreateOptions::default(),
            progress,
            control,
        ),
        _ => panic!("unknown test operation"),
    }
}

#[test]
fn archive_operations_report_opening_before_entry_work() {
    let temp = TempDir::new("opening-progress");
    let source = temp.path().join("payload.bin");
    fs::write(&source, vec![0x42; 256 * 1024]).unwrap();
    for format in ["sqz", "zip"] {
        let archive = temp.path().join(format!("backup.{format}"));
        engine()
            .create(
                &archive,
                std::slice::from_ref(&source),
                &CreateOptions::default(),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        for operation in [
            "test",
            "extract",
            "extract_report",
            "guarded_extract",
            "convert",
        ] {
            let destination = temp.path().join(format!("{format}-{operation}.zip"));
            let progress = OpeningProgress::default();
            run_operation(
                operation,
                &archive,
                &destination,
                &progress,
                &ControlToken::default(),
            )
            .unwrap();
            let events = progress.events.lock().unwrap();
            assert_eq!(events[0], Event::Phase(ProgressPhase::ArchiveOpen));
            assert_eq!(events[1], Event::Bytes(0, 0, format!("backup.{format}")));
            let next = match operation {
                "test" => ProgressPhase::ArchiveTest,
                "convert" => ProgressPhase::ArchiveConvert,
                _ => ProgressPhase::ExtractEntries,
            };
            assert_eq!(events[2], Event::Phase(next), "{format}/{operation}");
            assert!(
                events.iter().any(
                    |event| matches!(event, Event::Bytes(done, total, _) if *done > 0 && *total > 0)
                ),
                "{format}/{operation}"
            );
            if operation.starts_with("extract") || operation == "guarded_extract" {
                assert_eq!(
                    fs::read(destination.join("payload.bin")).unwrap(),
                    fs::read(&source).unwrap()
                );
            } else if operation == "convert" {
                assert!(engine()
                    .test_summary(
                        &destination,
                        &OpenOptions::default(),
                        &squallz_format_api::SafetyLimits::default(),
                        &NoProgress,
                        &ControlToken::default()
                    )
                    .unwrap()
                    .is_ok());
            }
        }
    }
}

#[test]
fn cancellation_from_opening_feedback_preserves_existing_outputs() {
    let temp = TempDir::new("opening-cancel");
    let source = temp.path().join("payload.bin");
    fs::write(&source, b"new contents").unwrap();
    let archive = temp.path().join("backup.sqz");
    engine()
        .create(
            &archive,
            &[source],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
    for operation in [
        "test",
        "extract",
        "extract_report",
        "guarded_extract",
        "convert",
    ] {
        let destination = temp.path().join(format!("{operation}.zip"));
        let original = if operation == "convert" {
            destination.clone()
        } else {
            fs::create_dir(&destination).unwrap();
            destination.join("payload.bin")
        };
        fs::write(&original, b"keep existing contents").unwrap();
        let control = ControlToken::default();
        let progress = OpeningProgress {
            cancel: Some(control.clone()),
            ..Default::default()
        };
        let error =
            run_operation(operation, &archive, &destination, &progress, &control).unwrap_err();
        assert!(
            matches!(error, FormatError::Cancelled),
            "{operation}: {error}"
        );
        assert_eq!(fs::read(original).unwrap(), b"keep existing contents");
        assert_eq!(
            progress.events.lock().unwrap().as_slice(),
            [
                Event::Phase(ProgressPhase::ArchiveOpen),
                Event::Bytes(0, 0, "backup.sqz".into()),
            ]
        );
    }
}
