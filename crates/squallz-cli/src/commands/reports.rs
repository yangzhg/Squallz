use serde_json::{json, Value};
use squallz_core::api::{FormatError, RecoverySummary, TestReport};

use crate::errors::CliError;

pub(crate) fn print_pretty_json(value: &Value) -> Result<(), CliError> {
    let text = pretty_json_text(value)?;
    println!("{text}");
    Ok(())
}

fn pretty_json_text(value: &Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| FormatError::Other(format!("cannot serialize CLI JSON report: {e}")).into())
}

pub(crate) fn test_report_json(report: &TestReport) -> Value {
    json!({
        "ok": report.is_ok(),
        "entries_tested": report.entries_tested,
        "problems": &report.problems,
        "recovery": report.recovery.as_ref().map(recovery_summary_json),
    })
}

pub(crate) fn recovery_summary_json(summary: &RecoverySummary) -> Value {
    json!({
        "scheme": &summary.scheme,
        "block_size": summary.block_size,
        "total_blocks": summary.total_blocks,
        "data_shards": summary.data_shards,
        "parity_shards": summary.parity_shards,
        "recovery_blocks_available": summary.recovery_blocks_available,
        "damaged_blocks": summary.damaged_blocks,
        "repaired_blocks": summary.repaired_blocks,
        "unrepaired_blocks": summary.unrepaired_blocks,
        "repair_possible": summary.repair_possible,
    })
}
