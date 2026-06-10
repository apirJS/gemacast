use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

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
    let bin_dir = Path::new(&manifest_dir).join("bin");

    if !bin_dir.exists() {
        fs::create_dir_all(&bin_dir).expect("Failed to create bin directory");
    }

    // Check if all required files already exist
    let all_exist = files_to_copy.iter().all(|file| bin_dir.join(file).exists());
    if all_exist {
        return;
    }

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

    let status = Command::new("tar")
        .args([
            "-xf",
            zip_path.to_str().unwrap(),
            "-C",
            out_path.to_str().unwrap(),
        ])
        .status()
        .expect("Failed to execute tar");

    if !status.success() {
        panic!("Failed to extract platform-tools");
    }

    let extracted_dir = out_path.join("platform-tools");

    for file in files_to_copy {
        let src = extracted_dir.join(file);
        let dest = bin_dir.join(file);
        fs::copy(&src, &dest).unwrap_or_else(|_| panic!("Failed to copy {}", file));
    }

    println!(
        "cargo:warning=ADB successfully bundled in bin/ directory for {}",
        target_os
    );
}
