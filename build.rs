use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const RC_TEMPLATE: &str = r#"1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
{
    BLOCK "StringFileInfo"
    {
        BLOCK "040904B0"
        {
            VALUE "FileDescription", "Jot"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "Jot"
            VALUE "OriginalFilename", "Jot.exe"
            VALUE "ProductName", "Jot"
            VALUE "ProductVersion", "{version}"
        }
    }
    BLOCK "VarFileInfo"
    {
        VALUE "Translation", 0x409, 1200
    }
}
"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/jot.ico");
    println!("cargo:rerun-if-changed=assets/jot.manifest.xml");
    println!("cargo:rerun-if-changed=Cargo.toml");

    compile_resources();
}

fn compile_resources() {
    let var = |name: &str| env::var(name).unwrap();
    let root = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let assets = root.join("assets");
    let escape = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");
    let icon = escape(assets.join("jot.ico"));
    let manifest = escape(assets.join("jot.manifest.xml"));

    let script = RC_TEMPLATE
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

    let rc_exe = find_windows_sdk_binary("rc.exe").expect("Windows SDK rc.exe not found");
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

fn find_windows_sdk_binary(binary: &str) -> Option<PathBuf> {
    if let Ok(path_env) = env::var("PATH") {
        for dir in env::split_paths(&path_env) {
            let candidate = dir.join(binary);
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
        if let Some(found) = latest_sdk_binary_in(&root, arch, binary) {
            return Some(found);
        }
    }
    None
}

fn latest_sdk_binary_in(kits_bin: &Path, arch: &str, binary: &str) -> Option<PathBuf> {
    let mut versions: Vec<String> = fs::read_dir(kits_bin)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    versions.sort_by_key(|s| {
        s.split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect::<Vec<_>>()
    });
    for version in versions.into_iter().rev() {
        let candidate = kits_bin.join(&version).join(arch).join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
