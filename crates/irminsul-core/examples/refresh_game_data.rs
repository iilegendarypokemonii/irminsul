//! Regenerates the bundled game data after a game update. Run from this crate:
//! `cargo run --example refresh_game_data`
use anime_game_data::AnimeGameData;
use anyhow::Result;
use flate2::{Compression, write::GzEncoder};
use std::{fs::File, path::Path};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/game-data.json.gz");
    let mut db = AnimeGameData::new();
    db.update().await?;
    let mut writer = GzEncoder::new(File::create(&path)?, Compression::best());
    db.save_to_writer(&mut writer)?;
    writer.finish()?;
    println!("Wrote {}", path.display());
    Ok(())
}
