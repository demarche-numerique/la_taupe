use la_taupe::{
    analysis::{Analysis, Hint, Type},
    http::server,
    twoddoc::trust_service,
};
use serde_json::json;
use std::{env::args, path::Path};

fn main() {
    let args: Vec<String> = args().collect();

    if args.contains(&String::from("--help")) {
        println!("La taupe: a tool to analyze files");
        println!("--version to print the version");
        println!("--trusted-repositories-urls to print the trusted repository urls");
        println!("la_taupe file_path to analyze a file");
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
