use std::{env, io, path::Path};
fn main() -> io::Result<()> {
    let out = env::var_os("OUT_DIR").unwrap();
    std::fs::copy(
        "crates/irminsul-core/data/game-data.json.gz",
        Path::new(&out).join("game_data.gz"),
    )?;
    println!("cargo:rerun-if-changed=crates/irminsul-core/data/game-data.json.gz");
    if env::var_os("CARGO_CFG_WINDOWS").is_some() {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()?;
    }
    #[cfg(all(unix, feature = "static-libpcap"))]
    println!("cargo:rustc-link-lib=static=pcap");
    Ok(())
}
