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

/// GOOD keys a game-data snapshot can export. Optimizers compare these with
/// their own key lists to catch a snapshot that predates a game version.
#[derive(Debug, Default)]
pub struct GoodKeys {
    pub weapons: std::collections::BTreeSet<String>,
    pub characters: std::collections::BTreeSet<String>,
}

pub fn bundled_good_keys() -> anyhow::Result<GoodKeys> {
    good_keys_from_gz(&include_bytes!("../data/game-data.json.gz")[..])
}

/// Reads keys from a gzipped game-data snapshot such as `data/game-data.json.gz`.
pub fn good_keys_from_gz(gz: impl std::io::Read) -> anyhow::Result<GoodKeys> {
    #[derive(serde::Deserialize)]
    struct Named {
        name: String,
    }
    #[derive(serde::Deserialize)]
    struct Data {
        weapon_map: std::collections::HashMap<u32, Named>,
        character_map: std::collections::HashMap<u32, String>,
    }
    let data: Data = serde_json::from_reader(flate2::read::GzDecoder::new(gz))?;
    Ok(GoodKeys {
        weapons: data
            .weapon_map
            .values()
            .map(|w| good::to_good_key(&w.name))
            .collect(),
        characters: data
            .character_map
            .values()
            .map(|c| good::to_good_key(c))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_good_keys_include_new_content() {
        let keys = super::bundled_good_keys().unwrap();
        assert!(keys.weapons.contains("WintersHeavyHeart"));
        assert!(keys.characters.contains("Vodyanitsa"));
    }
}
