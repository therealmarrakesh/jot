use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/jot.rc");
    println!("cargo:rerun-if-changed=assets/jot.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let var = |name: &str| env::var(name).unwrap();
    let assets = PathBuf::from(var("CARGO_MANIFEST_DIR")).join("assets");
    let icon = assets
        .join("jot.ico")
        .display()
        .to_string()
        .replace('\\', "\\\\");
    let script = fs::read_to_string(assets.join("jot.rc"))
        .unwrap()
        .replace("{icon}", &icon)
        .replace("{version}", &var("CARGO_PKG_VERSION"))
        .replace("{major}", &var("CARGO_PKG_VERSION_MAJOR"))
        .replace("{minor}", &var("CARGO_PKG_VERSION_MINOR"))
        .replace("{patch}", &var("CARGO_PKG_VERSION_PATCH"));
    let resource = PathBuf::from(var("OUT_DIR")).join("jot.rc");
    fs::write(&resource, script).unwrap();
    embed_resource::compile(&resource, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
