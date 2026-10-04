use anyhow::{Result, bail, ensure};
use auto_artifactarium::r#gen::protos::{
    AvatarInfo, Equip, Item, Material, PropValue, Reliquary, Weapon,
};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerificationFixture {
    pub(crate) uid: String,
    pub(crate) avatars: Vec<FixtureAvatar>,
    pub(crate) items: Vec<FixtureItem>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct FixtureAvatar {
    id: u32,
    guid: u64,
    #[serde(rename = "type")]
    avatar_type: u32,
    level: u32,
    ascension: u32,
    #[serde(default)]
    skill_levels: BTreeMap<u32, u32>,
    #[serde(default)]
    equip_guids: Vec<u64>,
}

impl FixtureAvatar {
    pub(crate) fn into_avatar(self) -> AvatarInfo {
        let mut avatar = AvatarInfo {
            avatar_id: self.id,
            guid: self.guid,
            avatar_type: self.avatar_type,
            equip_guid_list: self.equip_guids,
            skill_level_map: self.skill_levels.into_iter().collect(),
            ..Default::default()
        };
        avatar.prop_map.insert(
            4001,
            PropValue {
                val: self.level as i64,
                ..Default::default()
            },
        );
        avatar.prop_map.insert(
            1002,
            PropValue {
                val: self.ascension as i64,
                ..Default::default()
            },
        );
        avatar
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct FixtureItem {
    id: u32,
    guid: u64,
    kind: String,
    #[serde(default)]
    level: Option<u32>,
    #[serde(default)]
    ascension: Option<u32>,
    #[serde(default)]
    refinement: Option<u32>,
    #[serde(default)]
    main_prop_id: Option<u32>,
    #[serde(default)]
    substat_ids: Vec<u32>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    locked: bool,
}

impl FixtureItem {
    pub(crate) fn into_item(self) -> Result<Item> {
        let FixtureItem {
            id,
            guid,
            kind,
            level,
            ascension,
            refinement,
            main_prop_id,
            substat_ids,
            count,
            locked,
        } = self;
        let mut item = Item {
            item_id: id,
            guid,
            ..Default::default()
        };
        match kind.as_str() {
            "weapon" => {
                ensure!(
                    level.is_some() && ascension.is_some() && refinement.is_some(),
                    "Weapon fixture requires level, ascension, and refinement."
                );
                ensure!(
                    main_prop_id.is_none() && substat_ids.is_empty() && count.is_none(),
                    "Weapon fixture contains incompatible fields."
                );
                let mut equip = Equip::new();
                equip.is_locked = locked;
                equip.set_weapon(Weapon {
                    level: level.unwrap(),
                    promote_level: ascension.unwrap(),
                    affix_map: [(1, refinement.unwrap().saturating_sub(1))]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                });
                item.set_equip(equip);
            }
            "artifact" => {
                ensure!(
                    level.is_some() && main_prop_id.is_some(),
                    "Artifact fixture requires level and mainPropId."
                );
                ensure!(
                    ascension.is_none() && refinement.is_none() && count.is_none(),
                    "Artifact fixture contains incompatible fields."
                );
                let mut equip = Equip::new();
                equip.is_locked = locked;
                equip.set_reliquary(Reliquary {
                    level: level.unwrap(),
                    main_prop_id: main_prop_id.unwrap(),
                    append_prop_id_list: substat_ids,
                    ..Default::default()
                });
                item.set_equip(equip);
            }
            "material" => {
                ensure!(count.is_some(), "Material fixture requires count.");
                ensure!(
                    level.is_none()
                        && ascension.is_none()
                        && refinement.is_none()
                        && main_prop_id.is_none()
                        && substat_ids.is_empty()
                        && !locked,
                    "Material fixture contains incompatible fields."
                );
                item.set_material(Material {
                    count: count.unwrap(),
                    ..Default::default()
                });
            }
            other => bail!("Unknown fixture item kind: {other}"),
        }
        Ok(item)
    }
}
