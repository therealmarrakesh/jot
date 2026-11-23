use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/jot.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    fs::write(out_dir.join("notices.txt"), THIRD_PARTY_NOTICES).unwrap();
    compile_resources(&out_dir);
}

fn compile_resources(out_dir: &Path) {
    let var = |name: &str| env::var(name).unwrap();
    let root = PathBuf::from(var("CARGO_MANIFEST_DIR"));
    let escape = |p: PathBuf| p.display().to_string().replace('\\', "\\\\");

    let manifest_path = out_dir.join("jot.manifest.xml");
    fs::write(&manifest_path, MANIFEST_XML).unwrap();

    let icon = escape(root.join("assets").join("jot.ico"));
    let manifest = escape(manifest_path);

    let script = RC_TEMPLATE
        .replace("{icon}", &icon)
        .replace("{manifest}", &manifest)
        .replace("{version}", &var("CARGO_PKG_VERSION"))
        .replace("{major}", &var("CARGO_PKG_VERSION_MAJOR"))
        .replace("{minor}", &var("CARGO_PKG_VERSION_MINOR"))
        .replace("{patch}", &var("CARGO_PKG_VERSION_PATCH"));

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

const MANIFEST_XML: &str = r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
    <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
        <security>
            <requestedPrivileges>
                <requestedExecutionLevel level="asInvoker" uiAccess="false" />
            </requestedPrivileges>
        </security>
    </trustInfo>
    <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
        <application>
            <!-- Windows 10 / 11 -->
            <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
        </application>
    </compatibility>
    <application xmlns="urn:schemas-microsoft-com:asm.v3">
        <windowsSettings>
            <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
            <heapType xmlns="http://schemas.microsoft.com/SMI/2020/WindowsSettings">SegmentHeap</heapType>
        </windowsSettings>
    </application>
    <dependency>
        <dependentAssembly>
            <assemblyIdentity
                type='win32'
                name='Microsoft.Windows.Common-Controls'
                version='6.0.0.0'
                processorArchitecture='*'
                publicKeyToken='6595b64144ccf1df'
            />
        </dependentAssembly>
    </dependency>
</assembly>
"#;

const THIRD_PARTY_NOTICES: &str = r#"Jot includes the following MIT-licensed third-party software:

- ropey 2.0.0-beta.1 (https://github.com/cessen/ropey)
  Copyright (c) 2023 Nathan Vegdahl

- str_indices 0.4.4 (https://github.com/cessen/str_indices)
  Copyright (c) 2023 Nathan Vegdahl

- unicode-segmentation 1.13.3 (https://github.com/unicode-rs/unicode-segmentation)
  Copyright (c) 2015 The Rust Project Developers

- windows 0.62.2 and supporting windows-* crates (https://github.com/microsoft/windows-rs)
  (windows-collections 0.3.2, windows-core 0.62.2, windows-future 0.3.2,
   windows-implement 0.60.2, windows-interface 0.59.3, windows-link 0.2.1,
   windows-numerics 0.3.1, windows-result 0.4.1, windows-strings 0.5.1,
   windows-threading 0.2.1)
  Copyright (c) Microsoft Corporation

================================================================================
MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"#;
