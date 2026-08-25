use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // --- Windows application resources (icon + ComCtl32 v6/DPI manifest) ---
    // rfd uses native task dialogs, which inherit the executable's application
    // icon. Compile the icon and manifest into one resource to keep every dialog
    // and the executable itself consistently branded.
    if target_os == "windows" {
        let manifest = embed_manifest::new_manifest("Gemacast.PC").to_string();
        let mut resources = tauri_winres::WindowsResource::new();
        resources
            .set_icon("../wix/assets/Product.ico")
            .set_manifest(&manifest)
            .compile()
            .expect("unable to embed Windows application resources");

        println!("cargo:rerun-if-changed=../wix/assets/Product.ico");
    }

    // --- Bundle ADB binaries ---
    //
    // Google only publishes x86_64 platform-tools for Linux. For aarch64 Linux
    // builds we use the unofficial AndroidIDEOfficial ARM64 release instead.
    let is_linux_arm64 = target_os == "linux" && target_arch == "aarch64";

    let (archive_name, files_to_copy) = match target_os.as_str() {
        "windows" => (
            "platform-tools-latest-windows.zip".to_string(),
            vec!["adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll"],
        ),
        "macos" => ("platform-tools-latest-darwin.zip".to_string(), vec!["adb"]),
        "linux" => {
            if is_linux_arm64 {
                (
                    "platform-tools-34.0.3-aarch64.tar.xz".to_string(),
                    vec!["adb"],
                )
            } else {
                ("platform-tools-latest-linux.zip".to_string(), vec!["adb"])
            }
        }
        _ => {
            println!(
                "cargo:warning=Unsupported OS for ADB bundling: {}",
                target_os
            );
            return;
        }
    };

    let download_url = if is_linux_arm64 {
        // Unofficial ARM64 Linux platform-tools from AndroidIDEOfficial
        format!(
            "https://github.com/AndroidIDEOfficial/platform-tools/releases/download/v34.0.3/{}",
            archive_name
        )
    } else {
        // Official Google platform-tools
        format!("https://dl.google.com/android/repository/{}", archive_name)
    };

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let adb_dir = Path::new(&manifest_dir);

    // Check if all required files already exist in the crate root
    let all_exist = files_to_copy.iter().all(|file| adb_dir.join(file).exists());
    if !all_exist {
        println!(
            "cargo:warning=Downloading Android platform-tools (ADB) for {} {}...",
            target_os, target_arch
        );

        let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
        let out_path = Path::new(&out_dir);
        let archive_path = out_path.join(&archive_name);

        let status = Command::new("curl")
            .args(["-L", "-o", archive_path.to_str().unwrap(), &download_url])
            .status()
            .expect("Failed to execute curl");

        if !status.success() {
            panic!("Failed to download platform-tools from {}", download_url);
        }

        println!("cargo:warning=Extracting ADB...");

        let status = if is_linux_arm64 {
            // ARM64 Linux archive is .tar.xz
            Command::new("tar")
                .args([
                    "-xJf",
                    archive_path.to_str().unwrap(),
                    "-C",
                    out_path.to_str().unwrap(),
                ])
                .status()
                .expect("Failed to execute tar")
        } else if target_os == "windows" {
            Command::new("tar")
                .args([
                    "-xf",
                    archive_path.to_str().unwrap(),
                    "-C",
                    out_path.to_str().unwrap(),
                ])
                .status()
                .expect("Failed to execute tar")
        } else {
            Command::new("unzip")
                .args([
                    "-q",
                    "-o",
                    archive_path.to_str().unwrap(),
                    "-d",
                    out_path.to_str().unwrap(),
                ])
                .status()
                .expect("Failed to execute unzip")
        };

        if !status.success() {
            panic!("Failed to extract platform-tools");
        }

        let extracted_dir = out_path.join("platform-tools");

        for file in &files_to_copy {
            let src = extracted_dir.join(file);
            let dest = adb_dir.join(file);
            fs::copy(&src, &dest).unwrap_or_else(|_| panic!("Failed to copy {}", file));
        }

        println!(
            "cargo:warning=ADB successfully bundled in crate root for {} {}",
            target_os, target_arch
        );
    }

    // Create empty placeholder stubs for filenames from OTHER platforms.
    // cargo-dist's `include` copies all listed files regardless of target,
    // so every name must exist or the copy step fails.
    let all_names = ["adb", "adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll"];
    for name in &all_names {
        let path = adb_dir.join(name);
        if !path.exists() {
            let _ = fs::File::create(&path);
        }
    }
}
