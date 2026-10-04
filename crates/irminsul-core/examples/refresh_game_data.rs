//! Regenerates the bundled game data after a game update. Run from this crate:
//! `cargo run --example refresh_game_data`
use anime_game_data::AnimeGameData;
use anyhow::Result;
use flate2::{Compression, write::GzEncoder};
use std::{fs::File, path::Path};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/game-data.json.gz");
    // Compiled in before this run, so it is the snapshot being replaced.
    let old = irminsul_core::bundled_good_keys()?;
    let mut db = AnimeGameData::new();
    db.update().await?;
    let mut writer = GzEncoder::new(File::create(&path)?, Compression::best());
    db.save_to_writer(&mut writer)?;
    writer.finish()?;
    let new = irminsul_core::good_keys_from_gz(File::open(&path)?)?;
    println!("Wrote {}", path.display());
    println!(
        "New weapons: {:?}",
        new.weapons.difference(&old.weapons).collect::<Vec<_>>()
    );
    println!(
        "New characters: {:?}",
        new.characters
            .difference(&old.characters)
            .collect::<Vec<_>>()
    );
    println!(
        "Next: update the data hash in IMPLEMENTATION.md, commit, push, and bump irminsul-core in the optimizer."
    );
    Ok(())
}
