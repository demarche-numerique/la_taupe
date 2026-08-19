// build.rs
use std::env;
use std::path::Path;
use std::process::Command;

/*
build script for the project
*/
fn main() {
    // taken from https://stackoverflow.com/questions/43753491/include-git-commit-hash-as-string-into-rust-program
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let git_hash = String::from_utf8(output.stdout).unwrap();
    println!("cargo:rustc-env=GIT_HASH={}", git_hash);

    let profile = env::var("PROFILE").unwrap_or_default();

    if profile == "release" {
        download_models_if_needed();
    } else {
        fake_download_models();
    }
}

/// Every model file embedded in the binary via include_bytes!.
const MODELS: [&str; 3] = [
    "pp-ocrv6_tiny_det.onnx",
    "pp-ocrv6_tiny_rec.onnx",
    "ppocrv6_dict.txt",
];

fn fake_download_models() {
    let models_dir = Path::new("models");

    // on cree les fichiers models vides si les fichiers n'existent pas
    for name in MODELS {
        let path = models_dir.join(name);
        if !path.exists() {
            std::fs::File::create(&path)
                .unwrap_or_else(|e| panic!("Failed to create {}: {}", name, e));
        }
    }
}

fn download_models_if_needed() {
    let models_dir = Path::new("models");

    let missing = MODELS.iter().any(|name| {
        let path = models_dir.join(name);
        !path.exists() || path.metadata().unwrap().len() == 0
    });

    if missing {
        println!("Downloading models...");

        let output = Command::new("bash")
            .arg("download-models.sh")
            .current_dir(".")
            .output()
            .expect("Failed to execute download-model.sh. Make sure bash is available and the script exists.");

        if !output.status.success() {
            eprintln!("STDOUT: {}", String::from_utf8_lossy(&output.stdout));
            eprintln!("STDERR: {}", String::from_utf8_lossy(&output.stderr));
            panic!(
                "download-model.sh failed with exit code: {:?}",
                output.status.code()
            );
        }

        println!("Models downloaded successfully");
    } else {
        println!("Models already exist and are not empty, skipping download");
    }
}
