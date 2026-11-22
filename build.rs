use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/jot.rc");
    println!("cargo:rerun-if-changed=assets/jot.ico");
    println!("cargo:rerun-if-changed=vendor/gpui/resources/windows/gpui.manifest.xml");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let var = |name: &str| env::var(name).unwrap();
    let root = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let assets = root.join("assets");
    let escape = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");
    let icon = escape(assets.join("jot.ico"));
    let manifest = escape(root.join("vendor/gpui/resources/windows/gpui.manifest.xml"));

    let script = fs::read_to_string(assets.join("jot.rc"))
        .unwrap()
        .replace("{icon}", &icon)
        .replace("{manifest}", &manifest)
        .replace("{version}", &var("CARGO_PKG_VERSION"))
        .replace("{major}", &var("CARGO_PKG_VERSION_MAJOR"))
        .replace("{minor}", &var("CARGO_PKG_VERSION_MINOR"))
        .replace("{patch}", &var("CARGO_PKG_VERSION_PATCH"));

    let out_dir = PathBuf::from(var("OUT_DIR"));
    let rc_path = out_dir.join("jot.rc");
    let res_path = out_dir.join("jot.res");
    fs::write(&rc_path, script).unwrap();

    let rc_exe = find_rc_exe().expect("Windows SDK rc.exe not found");
    let status = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_path)
        .arg(&rc_path)
        .status()
        .expect("failed to invoke rc.exe");
    assert!(status.success(), "rc.exe failed with status {status}");

    println!("cargo:rustc-link-arg-bins={}", res_path.display());
}

fn find_rc_exe() -> Option<PathBuf> {
    if let Ok(path_env) = env::var("PATH") {
        for dir in env::split_paths(&path_env) {
            let candidate = dir.join("rc.exe");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "x86",
        Ok("aarch64") => "arm64",
        _ => "x64",
    };

    let mut kit_roots = Vec::new();
    if let Ok(pf86) = env::var("ProgramFiles(x86)") {
        kit_roots.push(PathBuf::from(pf86).join(r"Windows Kits\10\bin"));
    }
    kit_roots.push(PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin"));

    for root in kit_roots {
        if let Some(found) = latest_rc_in(&root, arch) {
            return Some(found);
        }
    }
    None
}

fn latest_rc_in(kits_bin: &Path, arch: &str) -> Option<PathBuf> {
    let mut versions: Vec<PathBuf> = fs::read_dir(kits_bin)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect();
    versions.sort();
    for version_dir in versions.into_iter().rev() {
        let candidate = version_dir.join(arch).join("rc.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
