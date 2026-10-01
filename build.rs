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
    watch_inputs();

    let profile = env::var("PROFILE").unwrap_or_default();

    if profile == "release" {
        download_models_if_needed();
    } else {
        fake_download_models();
    }
}

/// Without any `rerun-if-changed`, Cargo reruns this script only when a file of the
/// package changes: after a commit that changes no file, GIT_HASH kept the previous
/// commit, and `--version` (which names the measurement results) lied. Watch the
/// current commit instead, and what the model download depends on.
fn watch_inputs() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=download-models.sh");
    println!("cargo:rerun-if-changed=models");

    let Ok(output) = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .output()
    else {
        return;
    };
    let git_dir = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if git_dir.is_empty() {
        return;
    }
    let git_dir = Path::new(&git_dir);

    // HEAD moves on a checkout; the branch ref, on a commit; packed refs, after a gc.
    // A watched path that does not exist would rerun the script on every build.
    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    if let Ok(content) = std::fs::read_to_string(&head) {
        if let Some(reference) = content.strip_prefix("ref: ") {
            let reference = git_dir.join(reference.trim());
            if reference.exists() {
                println!("cargo:rerun-if-changed={}", reference.display());
            }
        }
    }
    let packed = git_dir.join("packed-refs");
    if packed.exists() {
        println!("cargo:rerun-if-changed={}", packed.display());
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
