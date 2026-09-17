//! Compare both capture methods on the same live login. Output contains private inventory data.
use anyhow::{Context, Result, bail, ensure};
use irminsul_core::{CaptureController, CaptureMode, Snapshot, run_helper_if_requested};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};

fn write_json(output: &Path, name: &str, value: &impl serde::Serialize) -> Result<()> {
    std::fs::write(output.join(name), serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn collect_pair(
    modern: &CaptureController,
    compatibility: &CaptureController,
    output: &Path,
) -> Result<(Snapshot, Snapshot)> {
    let deadline = Instant::now() + Duration::from_secs(240);
    let mut announced = false;
    while Instant::now() < deadline {
        let states = [modern.state()?, compatibility.state()?];
        for state in &states {
            ensure!(state.capturing, "Capture stopped: {}", state.message);
        }
        if !announced && states.iter().all(|s| s.active_backend.is_some()) {
            write_json(output, "status.json", &json!({"phase": "running"}))?;
            announced = true;
        }
        if let [Some(a), Some(b)] = states.map(|s| s.snapshots.first().cloned()) {
            return Ok((
                modern.snapshot(&a.uid, &a.capture_id)?,
                compatibility.snapshot(&b.uid, &b.capture_id)?,
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!(
        "No complete pair within four minutes. Packet Monitor: {}. Winsock: {}.",
        modern.state()?.message,
        compatibility.state()?.message
    )
}

fn normalized_category(good: &Value, category: &str) -> Value {
    let value = good[category].clone();
    if let Some(items) = value.as_array() {
        let mut items = items.clone();
        items.sort_by_cached_key(Value::to_string);
        Value::Array(items)
    } else {
        value
    }
}

fn compare(output: &Path, a: Snapshot, b: Snapshot) -> Result<()> {
    write_json(output, "packet-monitor.json", &a)?;
    write_json(output, "winsock.json", &b)?;
    let mut a_guids = a.artifact_guids.clone();
    let mut b_guids = b.artifact_guids.clone();
    a_guids.sort();
    b_guids.sort();
    let categories = ["artifacts", "characters", "weapons", "materials"];
    let matches = categories.map(|key| {
        (
            key,
            normalized_category(&a.good, key) == normalized_category(&b.good, key),
        )
    });
    let passed = a.summary.uid == b.summary.uid
        && a_guids == b_guids
        && matches.iter().all(|(_, same)| *same);
    let report = json!({
        "phase": if passed { "passed" } else { "mismatch" },
        "sameUid": a.summary.uid == b.summary.uid,
        "artifactGuidsMatch": a_guids == b_guids,
        "categoriesMatch": matches.into_iter().map(|(key, same)| (key.to_owned(), json!(same))).collect::<serde_json::Map<_, _>>(),
        "packetMonitorCounts": a.summary.counts,
        "winsockCounts": b.summary.counts,
    });
    write_json(output, "comparison.json", &report)?;
    write_json(output, "status.json", &report)?;
    ensure!(
        passed,
        "Captured inventories differ. Inspect the private comparison output."
    );
    Ok(())
}

fn run(output: &Path) -> Result<()> {
    let mut modern = CaptureController::new()?;
    let mut compatibility = CaptureController::new()?;
    modern.start_with_mode(CaptureMode::PacketMonitor)?;
    let result = compatibility
        .start_with_mode(CaptureMode::Compatibility)
        .and_then(|_| collect_pair(&modern, &compatibility, output));
    let stopped = [modern.stop_and_wait(), compatibility.stop_and_wait()];
    let (a, b) = result?;
    for result in stopped {
        result?;
    }
    compare(output, a, b)
}

fn main() -> Result<()> {
    if let Some(result) = run_helper_if_requested() {
        return result;
    }
    let output = std::env::args()
        .nth(1)
        .context("Pass a private output directory.")?;
    let output = Path::new(&output);
    std::fs::create_dir_all(output)?;
    write_json(output, "status.json", &json!({"phase": "starting"}))?;
    let result = run(output);
    if let Err(error) = &result {
        write_json(
            output,
            "status.json",
            &json!({"phase": "error", "message": format!("{error:#}")}),
        )?;
    }
    result
}
