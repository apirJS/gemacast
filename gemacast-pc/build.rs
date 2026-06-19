use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    // --- Windows application manifest (ComCtl32 v6 + DPI awareness) ---
    // This fixes the "TaskDialogIndirect entry point not found" crash
    // that occurs when tray-icon tries to use comctl32.dll v6 features.
    if target_os == "windows" {
        embed_manifest::embed_manifest(embed_manifest::new_manifest("Gemacast.PC"))
            .expect("unable to embed Windows application manifest");
    }

    // --- Bundle ADB binaries ---
    let (zip_name, files_to_copy) = match target_os.as_str() {
        "windows" => (
            "platform-tools-latest-windows.zip",
            vec!["adb.exe", "AdbWinApi.dll", "AdbWinUsbApi.dll"],
        ),
        "macos" => ("platform-tools-latest-darwin.zip", vec!["adb"]),
        "linux" => ("platform-tools-latest-linux.zip", vec!["adb"]),
        _ => {
            println!(
                "cargo:warning=Unsupported OS for ADB bundling: {}",
                target_os
            );
            return;
        }
    };

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let adb_dir = Path::new(&manifest_dir);

    // Check if all required files already exist in the crate root
    let all_exist = files_to_copy.iter().all(|file| adb_dir.join(file).exists());
    if !all_exist {
        println!(
            "cargo:warning=Downloading Android platform-tools (ADB) for {}...",
            target_os
        );

        let out_dir = env::var("OUT_DIR").expect("OUT_DIR not set");
        let out_path = Path::new(&out_dir);
        let zip_path = out_path.join(zip_name);
        let download_url = format!("https://dl.google.com/android/repository/{}", zip_name);

        let status = Command::new("curl")
            .args(["-L", "-o", zip_path.to_str().unwrap(), &download_url])
            .status()
            .expect("Failed to execute curl");

        if !status.success() {
            panic!("Failed to download platform-tools");
        }

        println!("cargo:warning=Extracting ADB...");

        let status = if target_os == "windows" {
            Command::new("tar")
                .args([
                    "-xf",
                    zip_path.to_str().unwrap(),
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
                    zip_path.to_str().unwrap(),
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
            "cargo:warning=ADB successfully bundled in crate root for {}",
            target_os
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
