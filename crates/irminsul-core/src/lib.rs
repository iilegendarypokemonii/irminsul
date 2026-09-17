//! Native capture and UID-scoped snapshots. No optimizer or GUI dependency.
mod capture_mode;
mod compatibility;
mod export;
pub mod good;
#[cfg(windows)]
mod native_capture;
pub mod player_data;
mod process;
mod session;
mod transport;
pub use capture_mode::{CaptureBackend, CaptureMode};
pub use session::{CaptureState, Counts, DataSelection, Engine, Snapshot, SnapshotSummary};
pub use transport::{CaptureController, run_helper_if_requested};

pub fn game_data() -> anyhow::Result<anime_game_data::AnimeGameData> {
    anime_game_data::AnimeGameData::new_from_reader(flate2::read::GzDecoder::new(
        &include_bytes!("../data/game-data.json.gz")[..],
    ))
}
