use crate::{Snapshot, good::to_good_key, player_data::ExportSettings};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Deserialize)]
struct WeaponInfo {
    name: String,
    rarity: u32,
}
#[derive(Deserialize)]
struct WeaponData {
    weapon_map: HashMap<u32, WeaponInfo>,
}

fn weapon_rarities() -> Result<HashMap<String, u32>> {
    let data: WeaponData = serde_json::from_reader(flate2::read::GzDecoder::new(
        &include_bytes!("../data/game-data.json.gz")[..],
    ))?;
    Ok(data
        .weapon_map
        .into_values()
        .map(|weapon| (to_good_key(&weapon.name), weapon.rarity))
        .collect())
}

fn at_least(record: &Value, field: &str, minimum: u32) -> bool {
    record[field]
        .as_u64()
        .is_some_and(|value| value >= u64::from(minimum))
}

fn filter(good: &mut Value, key: &str, keep: impl Fn(&Value) -> bool) {
    if let Some(records) = good.get_mut(key).and_then(Value::as_array_mut) {
        records.retain(keep);
    }
}

impl Snapshot {
    /// Settings alter a copy of the completed snapshot, never capture state.
    pub fn export_with_settings(&self, settings: &ExportSettings) -> Result<Value> {
        let selection = crate::DataSelection {
            artifacts: settings.include_artifacts,
            characters: settings.include_characters,
            weapons: settings.include_weapons,
            materials: settings.include_materials,
        };
        let mut good = self.export(&selection)?;
        filter(&mut good, "characters", |c| {
            at_least(c, "level", settings.min_character_level)
                && at_least(c, "ascension", settings.min_character_ascension)
                && at_least(c, "constellation", settings.min_character_constellation)
        });
        filter(&mut good, "artifacts", |a| {
            at_least(a, "level", settings.min_artifact_level)
                && at_least(a, "rarity", settings.min_artifact_rarity)
        });
        if let Some(weapons) = good.get_mut("weapons").and_then(Value::as_array_mut) {
            let rarities = weapon_rarities()?;
            for weapon in weapons.iter() {
                let key = weapon["key"].as_str().context("Weapon key missing")?;
                anyhow::ensure!(
                    rarities.contains_key(key),
                    "Unknown weapon {key}; its rarity cannot be checked."
                );
            }
            weapons.retain(|w| {
                at_least(w, "level", settings.min_weapon_level)
                    && at_least(w, "ascension", settings.min_weapon_ascension)
                    && at_least(w, "refinement", settings.min_weapon_refinement)
                    && rarities[w["key"].as_str().unwrap()] >= settings.min_weapon_rarity
            });
        }
        if settings.fake_initialize_4th_line {
            if let Some(artifacts) = good.get_mut("artifacts").and_then(Value::as_array_mut) {
                artifacts.iter_mut().for_each(simulate_fourth_stat);
            }
        }
        good["irminsul"]["settings"] = serde_json::to_value(settings)?;
        Ok(good)
    }
}

fn simulate_fourth_stat(artifact: &mut Value) {
    if artifact["rarity"] != 5 || !artifact["level"].as_u64().is_some_and(|level| level < 4) {
        return;
    }
    let Some(stats) = artifact["substats"].as_array() else {
        return;
    };
    let active: Vec<_> = stats
        .iter()
        .filter(|s| s["value"].as_f64().is_some_and(|v| v > 0.))
        .cloned()
        .collect();
    if active.len() != 3 {
        return;
    }
    let Some(pending) = artifact
        .get_mut("unactivatedSubstats")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if pending.is_empty() {
        return;
    }
    let fourth = pending.remove(0);
    let mut active = active;
    active.push(fourth);
    artifact["substats"] = Value::Array(active);
    artifact["level"] = 4.into();
    artifact["totalRolls"] = 4.into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Counts, SnapshotSummary};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn snapshot() -> Snapshot {
        Snapshot {
            summary: SnapshotSummary {
                uid: "100000001".into(),
                capture_id: "one".into(),
                captured_at_ms: 1,
                counts: Counts::default(),
                warnings: vec![],
            },
            artifact_guids: vec!["1".into()],
            unmapped_materials: BTreeMap::new(),
            good: json!({ "format": "GOOD", "version": 3, "source": "Irminsul",
                "characters": [{"key": "Amber", "level": 40, "ascension": 2, "constellation": 3}],
                "weapons": [{"key": "FavoniusWarbow", "level": 40, "ascension": 2, "refinement": 3}],
                "artifacts": [{"level": 0, "rarity": 5, "totalRolls": 3,
                    "substats": [{"key": "critRate_", "value": 3.9}, {"key": "critDMG_", "value": 7.8}, {"key": "atk_", "value": 5.8}],
                    "unactivatedSubstats": [{"key": "enerRech_", "value": 6.5}]}],
                "materials": {"Mora": 100}
            }),
        }
    }

    #[test]
    fn thresholds_include_boundary_and_exclude_below() -> Result<()> {
        for (category, setting, value) in [
            ("characters", "min_character_level", 40),
            ("characters", "min_character_ascension", 2),
            ("characters", "min_character_constellation", 3),
            ("artifacts", "min_artifact_level", 0),
            ("artifacts", "min_artifact_rarity", 4),
            ("weapons", "min_weapon_level", 40),
            ("weapons", "min_weapon_ascension", 2),
            ("weapons", "min_weapon_refinement", 3),
            ("weapons", "min_weapon_rarity", 4),
        ] {
            let mut snapshot = snapshot();
            if setting == "min_artifact_rarity" {
                snapshot.good["artifacts"][0]["rarity"] = 4.into();
            }
            let mut settings = serde_json::to_value(ExportSettings::default())?;
            settings[setting] = value.into();
            assert_eq!(
                snapshot.export_with_settings(&serde_json::from_value(settings.clone())?)?
                    [category]
                    .as_array()
                    .unwrap()
                    .len(),
                1,
                "{setting}"
            );
            settings[setting] = (value + 1).into();
            assert_eq!(
                snapshot.export_with_settings(&serde_json::from_value(settings)?)?[category]
                    .as_array()
                    .unwrap()
                    .len(),
                0,
                "{setting}"
            );
        }
        Ok(())
    }

    #[test]
    fn simulation_preserves_capture_and_filters_actual_level() -> Result<()> {
        let snapshot = snapshot();
        let original = snapshot.good.clone();
        let mut settings = ExportSettings::default();
        assert_eq!(
            snapshot.export_with_settings(&settings)?["artifacts"],
            original["artifacts"]
        );
        settings.fake_initialize_4th_line = true;
        let result = snapshot.export_with_settings(&settings)?;
        assert_eq!(result["artifacts"][0]["level"], 4);
        assert_eq!(result["artifacts"][0]["totalRolls"], 4);
        assert_eq!(
            result["artifacts"][0]["substats"].as_array().unwrap().len(),
            4
        );
        assert_eq!(result["artifacts"][0]["unactivatedSubstats"], json!([]));
        assert_eq!(snapshot.good, original);
        settings.min_artifact_level = 4;
        assert_eq!(
            snapshot.export_with_settings(&settings)?["artifacts"],
            json!([])
        );
        Ok(())
    }

    #[test]
    fn simulation_skips_ineligible_artifacts() -> Result<()> {
        for (field, value) in [
            ("rarity", json!(4)),
            ("level", json!(4)),
            ("unactivatedSubstats", json!([])),
            ("substats", json!([])),
        ] {
            let mut snapshot = snapshot();
            snapshot.good["artifacts"][0][field] = value;
            let result = snapshot.export_with_settings(&ExportSettings {
                fake_initialize_4th_line: true,
                ..Default::default()
            })?;
            assert_eq!(result["artifacts"], snapshot.good["artifacts"]);
        }
        Ok(())
    }

    #[test]
    fn selection_omits_categories_and_does_not_check_deselected_weapons() -> Result<()> {
        let mut snapshot = snapshot();
        snapshot.good["weapons"][0]["key"] = "FutureWeapon".into();
        let settings: ExportSettings = crate::DataSelection::default().into();
        let result = snapshot.export_with_settings(&settings)?;
        for category in ["weapons", "characters", "materials"] {
            assert!(result.get(category).is_none());
        }
        assert!(
            snapshot
                .export_with_settings(&ExportSettings::default())
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn old_settings_restore_with_simulation_off() -> Result<()> {
        let settings: ExportSettings = serde_json::from_value(json!({"min_artifact_rarity": 5}))?;
        assert_eq!(settings.min_artifact_rarity, 5);
        assert!(!settings.fake_initialize_4th_line);
        assert!(settings.include_characters);
        Ok(())
    }
}
