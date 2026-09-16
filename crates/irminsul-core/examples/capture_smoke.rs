use anyhow::{Result, ensure};
use irminsul_core::{CaptureController, run_helper_if_requested};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    if let Some(result) = run_helper_if_requested() {
        return result;
    }
    let mut controller = CaptureController::new()?;
    let mode = std::env::args()
        .nth(1)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or_default();
    controller.start_with_mode(mode)?;
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut previous_phase = String::new();
    loop {
        let state = controller.state()?;
        if previous_phase != state.phase {
            println!("Phase: {} ? {}", state.phase, state.message);
            previous_phase = state.phase.clone();
        }
        ensure!(state.phase != "error", "{}", state.message);
        ensure!(
            Instant::now() < deadline,
            "Capture never became ready: {}",
            state.message
        );
        if state.phase == "waiting" {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("Capture ready; observing for two seconds without a login.");
    std::thread::sleep(Duration::from_secs(2));
    controller.stop_and_wait()?;
    let state = controller.state()?;
    ensure!(
        !state.capturing && state.phase == "idle",
        "Capture did not stop cleanly: {}",
        state.message
    );
    ensure!(
        state.snapshots.is_empty(),
        "A snapshot appeared without a fresh login"
    );
    println!("PASS: helper ready, clean shutdown, no stale snapshot.");
    Ok(())
}
