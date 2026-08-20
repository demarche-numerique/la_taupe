use la_taupe::{
    analysis::{Analysis, Hint, Type},
    http::server,
    twoddoc::trust_service,
};
use serde_json::json;
use std::{
    env::args,
    path::{Path, PathBuf},
    time::Instant,
};

fn main() {
    let args: Vec<String> = args().collect();

    if args.contains(&String::from("--help")) {
        println!("La taupe: a tool to analyze files");
        println!("--version to print the version");
        println!("--trusted-repositories-urls to print the trusted repository urls");
        println!("la_taupe file_path to analyze a file as a 2D-Doc");
        println!("la_taupe --rib [--jobs N] path... to analyze files or directories as RIBs,");
        println!("        one JSON line per file, with the analysis duration");
        println!("la_taupe to start the server");
        std::process::exit(0);
    }

    if args.contains(&String::from("--version")) {
        println!("Version: {}", env!("GIT_HASH"));
        std::process::exit(0);
    }

    if args.contains(&String::from("--trusted-repositories-urls")) {
        let urls = trust_service::trusted_repositories_urls();
        for url in urls {
            println!("{}", url);
        }
        std::process::exit(0);
    }

    if args.contains(&String::from("--rib")) {
        env_logger::init();
        analyze_ribs(&args);
        return;
    }

    if args.len() == 1 {
        let _ = server::main();
    } else {
        env_logger::init();

        let paths: Vec<&Path> = args[1..].iter().map(Path::new).collect();
        paths.iter().for_each(|path| {
            let result = Analysis::try_from((*path, Some(Hint::Type(Type::Twoddoc))));
            match result {
                Ok(analysis_result) => {
                    let json = json!({
                        "file_path": path.to_str().unwrap(),
                        "analysis": analysis_result
                    });
                    println!("{}", serde_json::to_string_pretty(&json).unwrap());
                }
                Err(msg) => {
                    let json = json!({
                        "file_path": path.to_str().unwrap(),
                        "error": msg
                    });
                    println!("{}", serde_json::to_string_pretty(&json).unwrap());
                }
            }
        });
    }
}

/// Batch RIB analysis: expands directories, analyzes `--jobs` files concurrently and
/// prints one compact JSON line per file, so a driving script can stream the results.
fn analyze_ribs(args: &[String]) {
    let jobs = args
        .iter()
        .position(|arg| arg == "--jobs")
        .and_then(|i| args.get(i + 1))
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);

    let mut files: Vec<PathBuf> = Vec::new();
    let mut jobs_value = false;
    for arg in &args[1..] {
        if jobs_value {
            jobs_value = false;
            continue;
        }
        if arg == "--jobs" {
            jobs_value = true;
            continue;
        }
        if arg.starts_with("--") {
            continue;
        }

        match expand(Path::new(arg)) {
            Ok(mut paths) => files.append(&mut paths),
            Err(msg) => {
                eprintln!("{}", msg);
                std::process::exit(1);
            }
        }
    }

    if files.is_empty() {
        eprintln!("--rib expects at least one file or directory");
        std::process::exit(1);
    }

    let jobs = jobs.min(files.len());

    std::thread::scope(|scope| {
        for job in 0..jobs {
            let files = &files;

            scope.spawn(move || {
                files.iter().skip(job).step_by(jobs).for_each(|path| {
                    // println! locks stdout for the whole line: no interleaving
                    println!("{}", analyze_one_rib(path));
                });
            });
        }
    });
}

/// Files of a directory (hidden ones excluded), sorted so two runs list them in the
/// same order; a plain file is returned as is.
fn expand(path: &Path) -> Result<Vec<PathBuf>, String> {
    if !path.is_dir() {
        return Ok(vec![path.to_path_buf()]);
    }

    let entries =
        std::fs::read_dir(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| !name.starts_with('.'))
                .unwrap_or(false)
        })
        .collect();

    files.sort();

    Ok(files)
}

/// One stubborn file must not sink a whole directory run: panics are caught and
/// reported as an error line, like any other failure.
fn analyze_one_rib(path: &Path) -> String {
    let file_path = path.display().to_string();

    let started = Instant::now();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        Analysis::try_from((path, Some(Hint::Type(Type::Rib))))
    }));
    let duration_ms = started.elapsed().as_millis() as u64;

    let line = match outcome {
        Ok(Ok(analysis)) => {
            json!({"file_path": file_path, "duration_ms": duration_ms, "analysis": analysis})
        }
        Ok(Err(msg)) => json!({"file_path": file_path, "duration_ms": duration_ms, "error": msg}),
        Err(_) => {
            json!({"file_path": file_path, "duration_ms": duration_ms, "error": "analysis panicked"})
        }
    };

    serde_json::to_string(&line).unwrap()
}
