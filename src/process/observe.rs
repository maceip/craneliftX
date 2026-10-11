//! Uniform operation spans. Group assignment is shared with all ledger consumers.
use super::Result;
use crate::ledger::RunLedger;
use serde_json::{json, Value};
use std::time::Instant;

pub(super) fn operation<T>(
    ledger: &mut RunLedger,
    stage: &str,
    mut context: Value,
    action: impl FnOnce(&mut RunLedger) -> Result<T>,
    details: impl FnOnce(&T) -> Value,
) -> Result<T> {
    context["stage"] = json!(stage);
    context["status"] = json!("started");
    ledger.event("stage", context.clone())?;
    let start = Instant::now();
    let result = action(ledger);
    context["elapsed_ns"] = json!(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
    context["status"] = json!(if result.is_ok() {
        "completed"
    } else {
        "failed"
    });
    match &result {
        Ok(value) => context.as_object_mut().unwrap().extend(
            details(value)
                .as_object()
                .expect("operation details")
                .clone(),
        ),
        Err(error) => context["error"] = json!(error.to_string()),
    }
    ledger.event("stage", context)?;
    result
}
