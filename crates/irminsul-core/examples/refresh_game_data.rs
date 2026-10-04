//! Regenerates the bundled game data after a game update. Run from this crate:
//! `cargo run --example refresh_game_data`
use anime_game_data::AnimeGameData;
use anyhow::{Result, bail};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde_json::Value;
use std::{fs::File, path::Path};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("data/game-data.json.gz");
    let old_keys = irminsul_core::bundled_good_keys()?;
    let old: Value = serde_json::from_reader(GzDecoder::new(File::open(&path)?))?;
    let mut db = AnimeGameData::new();
    db.update().await?;
    let mut buf = Vec::new();
    db.save_to_writer(&mut buf)?;
    let mut new: Value = serde_json::from_slice(&buf)?;

    // The game data obfuscates field names between versions (7.1 renamed the
    // ConstValue `value` field), which anime-game-data reports as missing data.
    for (field, old_value) in old.as_object().unwrap() {
        let new_value = &mut new[field];
        match (old_value, &*new_value) {
            (Value::Object(o), Value::Object(n)) if n.len() < o.len() => {
                bail!(
                    "{field} shrank from {} to {} entries; not writing",
                    o.len(),
                    n.len()
                )
            }
            (o, Value::Null) if !o.is_null() => {
                println!("WARNING: {field} is now missing; kept the previous value {o}");
                *new_value = o.clone();
            }
            _ => {}
        }
    }

    let mut writer = GzEncoder::new(File::create(&path)?, Compression::best());
    serde_json::to_writer_pretty(&mut writer, &new)?;
    writer.finish()?;
    let new_keys = irminsul_core::good_keys_from_gz(File::open(&path)?)?;
    println!("Wrote {}", path.display());
    println!(
        "New weapons: {:?}",
        new_keys
            .weapons
            .difference(&old_keys.weapons)
            .collect::<Vec<_>>()
    );
    println!(
        "New characters: {:?}",
        new_keys
            .characters
            .difference(&old_keys.characters)
            .collect::<Vec<_>>()
    );
    println!(
        "Next: update the data hash in IMPLEMENTATION.md, commit, push, and bump irminsul-core in the optimizer."
    );
    Ok(())
}
