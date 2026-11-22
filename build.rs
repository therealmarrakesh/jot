use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/jot.rc");
    println!("cargo:rerun-if-changed=assets/jot.ico");
    println!("cargo:rerun-if-changed=assets/jot.manifest.xml");
    println!("cargo:rerun-if-changed=Cargo.toml");

    compile_resources();

    #[cfg(not(debug_assertions))]
    shader_compilation::compile_shaders();
}

fn compile_resources() {
    let var = |name: &str| env::var(name).unwrap();
    let root = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let assets = root.join("assets");
    let escape = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");
    let icon = escape(assets.join("jot.ico"));
    let manifest = escape(assets.join("jot.manifest.xml"));

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

#[cfg(not(debug_assertions))]
mod shader_compilation {
    use super::find_windows_sdk_binary;
    use std::{
        fs,
        io::Write,
        path::{Path, PathBuf},
        process::{self, Command},
    };

    pub fn compile_shaders() {
        let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let shader_dir = manifest_dir.join("src/win32");
        let shader_path = shader_dir.join("shaders.hlsl");
        let color_shader_path = shader_dir.join("color_text_raster.hlsl");
        let alpha_correction_path = shader_dir.join("alpha_correction.hlsl");
        let out_dir = std::env::var("OUT_DIR").unwrap();

        println!("cargo:rerun-if-changed={}", shader_path.display());
        println!("cargo:rerun-if-changed={}", color_shader_path.display());
        println!("cargo:rerun-if-changed={}", alpha_correction_path.display());

        let fxc_path = find_fxc_compiler();
        let modules = [
            "quad",
            "shadow",
            "monochrome_sprite",
            "polychrome_sprite",
        ];

        let rust_binding_path = format!("{out_dir}/shaders_bytes.rs");
        if Path::new(&rust_binding_path).exists() {
            fs::remove_file(&rust_binding_path)
                .expect("Failed to remove existing Rust binding file");
        }
        for module in modules {
            compile_shader_for_module(
                module,
                &out_dir,
                &fxc_path,
                shader_path.to_str().unwrap(),
                &rust_binding_path,
            );
        }

        compile_shader_for_module(
            "emoji_rasterization",
            &out_dir,
            &fxc_path,
            color_shader_path.to_str().unwrap(),
            &rust_binding_path,
        );
    }

    fn find_fxc_compiler() -> PathBuf {
        if let Ok(path) = std::env::var("GPUI_FXC_PATH") {
            let path = PathBuf::from(path);
            if path.is_file() {
                return path;
            }
        }
        find_windows_sdk_binary("fxc.exe").expect("Windows SDK fxc.exe not found")
    }

    fn compile_shader_for_module(
        module: &str,
        out_dir: &str,
        fxc_path: &Path,
        shader_path: &str,
        rust_binding_path: &str,
    ) {
        let output_file = format!("{out_dir}/{module}_vs.h");
        let const_name = format!("{}_VERTEX_BYTES", module.to_uppercase());
        compile_shader_impl(
            fxc_path,
            &format!("{module}_vertex"),
            &output_file,
            &const_name,
            shader_path,
            "vs_4_1",
        );
        generate_rust_binding(&const_name, &output_file, rust_binding_path);

        let output_file = format!("{out_dir}/{module}_ps.h");
        let const_name = format!("{}_FRAGMENT_BYTES", module.to_uppercase());
        compile_shader_impl(
            fxc_path,
            &format!("{module}_fragment"),
            &output_file,
            &const_name,
            shader_path,
            "ps_4_1",
        );
        generate_rust_binding(&const_name, &output_file, rust_binding_path);
    }

    fn compile_shader_impl(
        fxc_path: &Path,
        entry_point: &str,
        output_path: &str,
        var_name: &str,
        shader_path: &str,
        target: &str,
    ) {
        let output = Command::new(fxc_path)
            .args([
                "/T",
                target,
                "/E",
                entry_point,
                "/Fh",
                output_path,
                "/Vn",
                var_name,
                "/O3",
                shader_path,
            ])
            .output();

        match output {
            Ok(result) => {
                if result.status.success() {
                    return;
                }
                println!(
                    "cargo::error=Shader compilation failed for {entry_point}:\n{}",
                    String::from_utf8_lossy(&result.stderr)
                );
                process::exit(1);
            }
            Err(e) => {
                println!("cargo::error=Failed to run fxc for {entry_point}: {e}");
                process::exit(1);
            }
        }
    }

    fn generate_rust_binding(const_name: &str, head_file: &str, output_path: &str) {
        let header_content = fs::read_to_string(head_file).expect("Failed to read header file");
        let const_definition = {
            let global_var_start = header_content.find("const BYTE").unwrap();
            let global_var = &header_content[global_var_start..];
            let equal = global_var.find('=').unwrap();
            global_var[equal + 1..].trim()
        };
        let rust_binding = format!(
            "const {const_name}: &[u8] = &{}\n",
            const_definition.replace('{', "[").replace('}', "]")
        );
        let mut options = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(output_path)
            .expect("Failed to open Rust binding file");
        options
            .write_all(rust_binding.as_bytes())
            .expect("Failed to write Rust binding file");
    }
}
