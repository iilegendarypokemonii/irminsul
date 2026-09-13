//! Native capture and UID-scoped snapshots. No optimizer or GUI dependency.
mod export;
pub mod good;
pub mod player_data;
mod process;
mod session;
mod transport;
pub use session::{CaptureState, Counts, DataSelection, Engine, Snapshot, SnapshotSummary};
pub use transport::{CaptureController, run_helper_if_requested};

pub fn game_data() -> anyhow::Result<anime_game_data::AnimeGameData> {
    anime_game_data::AnimeGameData::new_from_reader(flate2::read::GzDecoder::new(
        &include_bytes!("../data/game-data.json.gz")[..],
    ))
}
