//! Local regression replay. Pass normalized private recordings, never commit them.
use anyhow::{Context, Result, ensure};
use irminsul_core::Engine;
use pcap_file::pcapng::{PcapNgReader, blocks::Block};
use std::{fs::File, path::PathBuf};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 2,
        "Usage: replay output-directory process:uid:path [...]"
    );
    let out = PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out)?;
    let mut engine = Engine::new()?;
    engine.start()?;
    for (index, arg) in args[1..].iter().enumerate() {
        let mut parts = arg.splitn(3, ':');
        let process = parts.next().context("process")?;
        let uid = parts.next().context("uid")?;
        let path = parts.next().context("path")?;
        engine.observe_process(Some(process.to_string()))?;
        let mut reader = PcapNgReader::new(File::open(path)?)?;
        while let Some(block) = reader.next_block() {
            if let Block::EnhancedPacket(packet) = block? {
                engine.receive(packet.data.to_vec())?;
            }
        }
        let state = engine.state();
        ensure!(state.phase == "ready", "{}: {}", state.phase, state.message);
        ensure!(
            state.active_uid.as_deref() == Some(uid),
            "Captured UID differs from expected UID"
        );
        let summary = state
            .snapshots
            .iter()
            .find(|s| s.uid == uid)
            .context("Missing snapshot")?;
        let snapshot = engine.snapshot(uid, &summary.capture_id)?;
        std::fs::write(
            out.join(format!("{index}-{uid}.json")),
            serde_json::to_vec_pretty(&snapshot)?,
        )?;
        println!("{}", serde_json::to_string(summary)?);
    }
    Ok(())
}
