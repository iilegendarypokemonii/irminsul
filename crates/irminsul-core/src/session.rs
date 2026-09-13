use crate::{
    game_data,
    player_data::{ExportSettings, PlayerData},
};
use anyhow::{Context, Result, ensure};
use auto_artifactarium::r#gen::protos::{AvatarInfo, Item, Unk};
use auto_artifactarium::{
    GameCommand, GamePacket, GameSniffer, matches_avatar_packet, matches_item_packet,
};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use protobuf::{Message, UnknownValueRef};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSelection {
    pub artifacts: bool,
    pub characters: bool,
    pub weapons: bool,
    pub materials: bool,
}

impl Default for DataSelection {
    fn default() -> Self {
        Self {
            artifacts: true,
            characters: false,
            weapons: false,
            materials: false,
        }
    }
}

impl From<DataSelection> for ExportSettings {
    fn from(selection: DataSelection) -> Self {
        Self {
            include_artifacts: selection.artifacts,
            include_characters: selection.characters,
            include_weapons: selection.weapons,
            include_materials: selection.materials,
            ..all_settings()
        }
    }
}

impl DataSelection {
    pub fn any(&self) -> bool {
        self.artifacts || self.characters || self.weapons || self.materials
    }

    pub fn apply(&self, good: &Value) -> Result<Value> {
        ensure!(self.any(), "Select at least one data category.");
        let mut result = good.clone();
        let object = result.as_object_mut().context("Invalid snapshot format")?;
        for (key, selected) in [
            ("artifacts", self.artifacts),
            ("characters", self.characters),
            ("weapons", self.weapons),
            ("materials", self.materials),
        ] {
            if !selected {
                object.remove(key);
            }
        }
        Ok(result)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Counts {
    pub artifacts: usize,
    pub characters: usize,
    pub weapons: usize,
    pub materials: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub uid: String,
    pub capture_id: String,
    pub captured_at_ms: u64,
    pub counts: Counts,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    #[serde(flatten)]
    pub summary: SnapshotSummary,
    pub good: Value,
    /// Decimal strings, never floating point JavaScript numbers.
    pub artifact_guids: Vec<String>,
    pub unmapped_materials: BTreeMap<String, u32>,
}

impl Snapshot {
    pub fn export(&self, selection: &DataSelection) -> Result<Value> {
        let mut good = selection.apply(&self.good)?;
        good["irminsul"] = serde_json::json!({
            "uid": self.summary.uid, "captureId": self.summary.capture_id,
            "capturedAtMs": self.summary.captured_at_ms,
            "version": env!("CARGO_PKG_VERSION"), "gameDataRevision": "26df1dfbdf05a82bbb1d97506859f3e1c40718d8"
        });
        if selection.materials && !self.unmapped_materials.is_empty() {
            good["irminsul"]["unmappedMaterials"] = serde_json::to_value(&self.unmapped_materials)?;
        }
        Ok(good)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureState {
    pub capturing: bool,
    pub phase: String,
    pub message: String,
    pub active_uid: Option<String>,
    pub snapshots: Vec<SnapshotSummary>,
}

/// One engine per application. Only a decoding hint survives a login; completed
/// snapshots are separately keyed by their captured UID, never by a wish authkey.
pub struct Engine {
    keys: HashMap<u16, Vec<u8>>,
    sniffer: GameSniffer,
    player: PlayerData,
    process: Option<String>,
    hint: Option<u64>,
    endpoint: Option<String>,
    uid: Option<String>,
    capture_id: String,
    items: Option<Vec<Item>>,
    avatars: bool,
    snapshots: BTreeMap<String, Snapshot>,
    state: CaptureState,
    failed: bool,
}

impl Engine {
    pub fn new() -> Result<Self> {
        let encoded: HashMap<u16, String> =
            serde_json::from_slice(include_bytes!("../data/dispatch-keys.json"))?;
        let keys = encoded
            .into_iter()
            .map(|(id, key)| Ok((id, BASE64_STANDARD.decode(key)?)))
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(Self {
            sniffer: GameSniffer::new().set_initial_keys(keys.clone()),
            keys,
            player: PlayerData::new(game_data()?),
            process: None,
            hint: None,
            endpoint: None,
            uid: None,
            capture_id: String::new(),
            items: None,
            avatars: false,
            snapshots: BTreeMap::new(),
            failed: false,
            state: CaptureState {
                phase: "idle".into(),
                message: "Start capture before entering the game door.".into(),
                ..Default::default()
            },
        })
    }

    pub fn state(&self) -> CaptureState {
        let mut state = self.state.clone();
        state.active_uid = self.uid.clone();
        state.snapshots = self.snapshots.values().map(|s| s.summary.clone()).collect();
        state
    }

    pub fn status(&mut self, phase: &str, message: impl Into<String>) {
        self.state.phase = phase.into();
        self.state.message = message.into();
    }

    pub fn start(&mut self) -> Result<()> {
        self.reset_login()?;
        self.state.capturing = true;
        self.status("starting", "Waiting for capture permission…");
        Ok(())
    }

    pub fn stop(&mut self) {
        self.state.capturing = false;
        self.status(
            "idle",
            "Capture stopped. Completed snapshots remain available by account.",
        );
    }

    pub fn fail(&mut self, message: impl Into<String>) {
        self.failed = true;
        self.status("error", message);
    }

    /// A process identity must include creation time as well as PID to prevent reuse.
    pub fn observe_process(&mut self, process: Option<String>) -> Result<()> {
        if self.process == process {
            return Ok(());
        }
        self.process = process;
        self.hint = None;
        self.reset_login()?;
        self.status(
            "waiting",
            "Game process changed. Waiting for a fresh login.",
        );
        Ok(())
    }

    fn reset_login(&mut self) -> Result<()> {
        self.sniffer = GameSniffer::new()
            .set_initial_keys(self.keys.clone())
            .with_client_seed_hint(self.hint);
        self.player = PlayerData::new(game_data()?);
        self.endpoint = None;
        self.uid = None;
        self.items = None;
        self.avatars = false;
        self.failed = false;
        self.capture_id = random_id()?;
        Ok(())
    }

    pub fn receive(&mut self, packet: Vec<u8>) -> Result<()> {
        let Some(route) = packet_route(&packet) else {
            return Ok(());
        };
        if route.login {
            self.reset_login()?;
            self.endpoint = Some(route.endpoint.clone());
            self.status("identifying", "Login detected. Identifying this account…");
        }
        if self.failed || self.endpoint.as_ref() != Some(&route.endpoint) {
            return Ok(());
        }
        if let Some(GamePacket::Commands(commands)) = self.sniffer.receive_packet(packet) {
            for command in commands {
                self.command(command)?;
            }
        }
        if self.sniffer.key_search_failed() {
            self.fail("Could not decode this login. Fully restart the game with capture running.");
        }
        self.hint = self.sniffer.client_seed_hint().or(self.hint);
        Ok(())
    }

    fn command(&mut self, command: GameCommand) -> Result<()> {
        self.identify_command(&command)?;
        if self.uid.is_none() {
            return Ok(());
        }
        if command.is_player_store_notify() {
            let items = matches_item_packet(&command)
                .context("Could not read inventory. Update Irminsul before exporting.")?;
            self.accept_items(items)?;
        }
        if command.is_avatar_data_notify() {
            let avatars = matches_avatar_packet(&command)
                .context("Could not read characters. Update Irminsul before exporting.")?;
            self.accept_avatars(avatars)?;
        }
        Ok(())
    }

    fn identify_command(&mut self, command: &GameCommand) -> Result<()> {
        if command.is_login_response {
            self.identify(login_uid(&command.body_data)?)?;
        }
        Ok(())
    }

    fn identify(&mut self, uid: String) -> Result<()> {
        ensure!(
            self.uid.as_ref().is_none_or(|old| old == &uid),
            "Conflicting account identities in one login."
        );
        self.uid = Some(uid);
        self.status(
            "collecting",
            "Account identified. Waiting for inventory and equipment…",
        );
        Ok(())
    }

    fn accept_items(&mut self, items: Vec<Item>) -> Result<()> {
        validate_items(&items)?;
        self.player.process_items(&items);
        self.items = Some(items);
        self.complete()
    }

    fn accept_avatars(&mut self, avatars: Vec<AvatarInfo>) -> Result<()> {
        validate_avatars(&avatars)?;
        self.player.process_characters(&avatars);
        self.avatars = true;
        self.complete()
    }

    fn complete(&mut self) -> Result<()> {
        let (Some(uid), Some(items), true) = (&self.uid, &self.items, self.avatars) else {
            return Ok(());
        };
        let good: Value =
            serde_json::from_str(&self.player.export_genshin_optimizer(&all_settings())?)?;
        let guids: Vec<_> = items
            .iter()
            .filter(|i| i.has_equip() && i.equip().has_reliquary())
            .map(|i| i.guid.to_string())
            .collect();
        let counts = counts(&good);
        let weapon_count = items
            .iter()
            .filter(|item| item.has_equip() && item.equip().has_weapon())
            .count();
        ensure!(
            counts.weapons == weapon_count,
            "Some weapons could not be converted. Snapshot withheld."
        );
        ensure!(
            counts.artifacts == guids.len(),
            "Some artifacts could not be converted. Snapshot withheld."
        );
        ensure!(
            guids.iter().collect::<BTreeSet<_>>().len() == guids.len(),
            "Duplicate artifact identifiers. Snapshot withheld."
        );
        ensure!(
            self.snapshots.contains_key(uid) || self.snapshots.len() < 32,
            "Account snapshot limit reached. Restart Irminsul before scanning more accounts."
        );
        let unmapped_materials = unmapped_materials(items)?;
        let warnings = if unmapped_materials.is_empty() {
            vec![]
        } else {
            vec![format!(
                "{} material types have no bundled name. Their item IDs and quantities are preserved in export metadata. Update Irminsul to add their names.",
                unmapped_materials.len()
            )]
        };
        let summary = SnapshotSummary {
            uid: uid.clone(),
            capture_id: self.capture_id.clone(),
            captured_at_ms: now_ms(),
            counts,
            warnings,
        };
        self.snapshots.insert(
            uid.clone(),
            Snapshot {
                summary,
                good,
                artifact_guids: guids,
                unmapped_materials,
            },
        );
        self.status(
            "ready",
            "Snapshot ready. Select an account and the data to export.",
        );
        Ok(())
    }

    pub fn snapshot(&self, uid: &str, capture_id: &str) -> Result<Snapshot> {
        let snapshot = self
            .snapshots
            .get(uid)
            .context("No complete snapshot for this account.")?;
        ensure!(
            snapshot.summary.capture_id == capture_id,
            "A newer snapshot is available. Review it before exporting."
        );
        Ok(snapshot.clone())
    }
}

fn login_uid(body: &[u8]) -> Result<String> {
    let body = Unk::parse_from_bytes(body)?;
    let values: Vec<_> = body
        .unknown_fields()
        .iter()
        .filter_map(|(field, value)| match (field, value) {
            (4, UnknownValueRef::Varint(uid)) => Some(uid),
            _ => None,
        })
        .collect();
    ensure!(
        values.len() == 1,
        "This login has no unambiguous account UID. Snapshot withheld."
    );
    ensure!(
        (100_000_000..=u32::MAX as u64).contains(&values[0]),
        "Unsupported account UID. Snapshot withheld."
    );
    Ok(values[0].to_string())
}

fn counts(good: &Value) -> Counts {
    let count = |name| good.get(name).and_then(Value::as_array).map_or(0, Vec::len);
    Counts {
        artifacts: count("artifacts"),
        characters: count("characters"),
        weapons: count("weapons"),
        materials: good["materials"].as_object().map_or(0, |m| m.len()),
    }
}

fn unmapped_materials(items: &[Item]) -> Result<BTreeMap<String, u32>> {
    let db = game_data()?;
    Ok(items
        .iter()
        .filter(|item| item.has_material() && db.get_material(item.item_id).is_err())
        .map(|item| (item.item_id.to_string(), item.material().count))
        .collect())
}

pub(crate) fn all_settings() -> ExportSettings {
    ExportSettings {
        include_characters: true,
        include_artifacts: true,
        include_weapons: true,
        include_materials: true,
        fake_initialize_4th_line: false,
        min_character_level: 0,
        min_character_ascension: 0,
        min_character_constellation: 0,
        min_artifact_level: 0,
        min_artifact_rarity: 1,
        min_weapon_level: 0,
        min_weapon_refinement: 0,
        min_weapon_ascension: 0,
        min_weapon_rarity: 1,
    }
}

fn validate_items(items: &[Item]) -> Result<()> {
    let db = game_data()?;
    let mut guids = BTreeSet::new();
    for item in items {
        ensure!(
            item.item_id != 0 && item.guid != 0 && guids.insert(item.guid),
            "Invalid or duplicate item identity. Snapshot withheld."
        );
        if item.has_equip() && item.equip().has_weapon() {
            db.get_weapon(item.item_id)
                .context("Unknown weapon ID; update Irminsul before exporting.")?;
            let weapon = item.equip().weapon();
            ensure!(
                (1..=90).contains(&weapon.level) && weapon.promote_level <= 6,
                "Invalid weapon level. Snapshot withheld."
            );
            ensure!(
                weapon.affix_map.values().all(|value| *value <= 4),
                "Invalid weapon refinement. Snapshot withheld."
            );
        }
    }
    validate_artifacts(items, &db)
}

fn validate_artifacts(items: &[Item], db: &anime_game_data::AnimeGameData) -> Result<()> {
    for item in items
        .iter()
        .filter(|i| i.has_equip() && i.equip().has_reliquary())
    {
        db.get_artifact(item.item_id)
            .context("Unknown artifact ID; update Irminsul before exporting.")?;
        let artifact = item.equip().reliquary();
        ensure!(
            (1..=21).contains(&artifact.level),
            "Invalid artifact level. Snapshot withheld."
        );
        db.get_property(artifact.main_prop_id)
            .context("Unknown artifact main stat.")?;
        for affix in artifact
            .append_prop_id_list
            .iter()
            .chain(&artifact.unactivated_prop_id_list)
        {
            db.get_affix(*affix).context("Unknown artifact substat.")?;
        }
    }
    Ok(())
}

fn validate_avatars(avatars: &[AvatarInfo]) -> Result<()> {
    let db = game_data()?;
    let tps: Vec<_> = [db.get_tps_avatar_id_female(), db.get_tps_avatar_id_male()]
        .into_iter()
        .filter_map(Result::ok)
        .collect();
    let mut guids = BTreeSet::new();
    for avatar in avatars {
        ensure!(
            avatar.avatar_id != 0 && avatar.guid != 0 && guids.insert(avatar.guid),
            "Invalid or duplicate character identity. Snapshot withheld."
        );
        if avatar.avatar_type != 1 || tps.contains(&avatar.avatar_id) {
            continue;
        }
        db.get_character(avatar.avatar_id)
            .context("Unknown character ID; update Irminsul before exporting.")?;
        ensure!(
            avatar.prop_map.contains_key(&4001) && avatar.prop_map.contains_key(&1002),
            "Missing character level or ascension. Update Irminsul before exporting."
        );
    }
    Ok(())
}

pub(crate) fn random_id() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("Could not create capture identity."))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

struct PacketRoute {
    endpoint: String,
    login: bool,
}

pub(crate) fn is_login_packet(bytes: &[u8]) -> bool {
    packet_route(bytes).is_some_and(|route| route.login)
}

fn packet_route(bytes: &[u8]) -> Option<PacketRoute> {
    use etherparse::{NetSlice, SlicedPacket, TransportSlice};
    let packet = SlicedPacket::from_ethernet(bytes).ok()?;
    let TransportSlice::Udp(udp) = packet.transport? else {
        return None;
    };
    let (source, destination) = match packet.net? {
        NetSlice::Ipv4(ip) => (
            ip.header().source_addr().to_string(),
            ip.header().destination_addr().to_string(),
        ),
        NetSlice::Ipv6(ip) => (
            ip.header().source_addr().to_string(),
            ip.header().destination_addr().to_string(),
        ),
    };
    let (src, dst) = (udp.source_port(), udp.destination_port());
    let (local, remote, sent) = if [22101, 22102].contains(&dst) {
        (
            format!("{source}:{src}"),
            format!("{destination}:{dst}"),
            true,
        )
    } else if [22101, 22102].contains(&src) {
        (
            format!("{destination}:{dst}"),
            format!("{source}:{src}"),
            false,
        )
    } else {
        return None;
    };
    let payload = udp.payload();
    let login = sent && payload.len() == 20 && payload[..4] == 255u32.to_be_bytes();
    Some(PacketRoute {
        endpoint: format!("{local}/{remote}"),
        login,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_weapon_and_character_mappings_withhold_snapshot() -> Result<()> {
        use auto_artifactarium::r#gen::protos::{Equip, Weapon};
        let mut engine = Engine::new()?;
        engine.identify("100000001".into())?;
        engine.accept_avatars(vec![])?;
        let mut item = Item {
            item_id: u32::MAX,
            guid: 1,
            ..Default::default()
        };
        let mut equip = Equip::new();
        equip.set_weapon(Weapon {
            level: 1,
            ..Default::default()
        });
        item.set_equip(equip);
        assert!(
            engine
                .accept_items(vec![item])
                .unwrap_err()
                .to_string()
                .contains("Unknown weapon")
        );
        assert!(engine.state().snapshots.is_empty());
        engine.reset_login()?;
        engine.identify("100000001".into())?;
        engine.accept_items(vec![])?;
        let avatar = AvatarInfo {
            avatar_id: u32::MAX,
            guid: 1,
            avatar_type: 1,
            ..Default::default()
        };
        assert!(
            engine
                .accept_avatars(vec![avatar])
                .unwrap_err()
                .to_string()
                .contains("Unknown character")
        );
        assert!(engine.state().snapshots.is_empty());
        Ok(())
    }

    #[test]
    fn malformed_identity_and_missing_character_properties_are_rejected() -> Result<()> {
        assert!(validate_items(&[Item::new()]).is_err());
        assert!(validate_avatars(&[AvatarInfo::new()]).is_err());
        let avatar = AvatarInfo {
            avatar_id: 10000003,
            guid: 1,
            avatar_type: 1,
            ..Default::default()
        };
        assert!(
            validate_avatars(&[avatar])
                .unwrap_err()
                .to_string()
                .contains("Missing character level")
        );
        Ok(())
    }

    #[test]
    fn missing_identity_or_partial_snapshot_cannot_export() -> Result<()> {
        let mut engine = Engine::new()?;
        engine.start()?;
        engine.accept_items(vec![])?;
        assert!(engine.state().snapshots.is_empty());
        engine.identify("100000001".into())?;
        assert!(engine.state().snapshots.is_empty());
        engine.accept_avatars(vec![])?;
        assert_eq!(engine.state().snapshots.len(), 1);
        Ok(())
    }

    #[test]
    fn login_reset_retains_only_hint_and_completed_snapshots() -> Result<()> {
        let mut engine = Engine::new()?;
        engine.observe_process(Some("1:100".into()))?;
        engine.hint = Some(123);
        engine.identify("100000001".into())?;
        engine.accept_items(vec![])?;
        engine.accept_avatars(vec![])?;
        engine.reset_login()?;
        assert_eq!(engine.hint, Some(123));
        assert!(engine.uid.is_none() && engine.items.is_none() && !engine.avatars);
        assert_eq!(engine.state().snapshots.len(), 1);
        engine.observe_process(Some("1:200".into()))?;
        assert!(engine.hint.is_none());
        Ok(())
    }

    #[test]
    fn conflicting_identity_and_stale_capture_are_rejected() -> Result<()> {
        let mut engine = Engine::new()?;
        engine.start()?;
        engine.identify("100000001".into())?;
        assert!(engine.identify("100000002".into()).is_err());
        engine.accept_items(vec![])?;
        engine.accept_avatars(vec![])?;
        assert!(engine.snapshot("100000001", "old").is_err());
        assert!(engine.snapshot("100000002", &engine.capture_id).is_err());
        Ok(())
    }

    #[test]
    fn selection_omits_unselected_categories_and_rejects_empty() -> Result<()> {
        let good = serde_json::json!({"format":"GOOD","artifacts":[],"characters":[],"weapons":[],"materials":{}});
        let selected = DataSelection::default().apply(&good)?;
        assert!(selected.get("artifacts").is_some());
        assert!(selected.get("characters").is_none() && selected.get("materials").is_none());
        let empty = DataSelection {
            artifacts: false,
            ..Default::default()
        };
        assert!(empty.apply(&good).is_err());
        Ok(())
    }
}
