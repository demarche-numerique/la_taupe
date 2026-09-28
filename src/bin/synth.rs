//! Génère un corpus de RIB synthétiques et sa vérité terrain.
//!
//!     cargo run --release --features bench --bin synth -- --out <dir> [--seed N] [--count N]
//!     cargo run --release --features bench --bin synth -- --out <dir> --from <dir> --truth <csv> [--seed N]

use std::path::PathBuf;
use std::process::exit;

use la_taupe::synth;

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.contains(&String::from("--help")) || args.len() == 1 {
        println!("Génère un corpus de RIB synthétiques et sa vérité terrain.");
        println!();
        println!("  --out <dir>    dossier de destination (obligatoire)");
        println!("  --seed <n>     graine du tirage, 1 par défaut");
        println!("  --count <n>    limite le nombre de documents produits");
        println!("  --from <dir>   photographie les documents de ce dossier au lieu des gabarits");
        println!("  --truth <csv>  vérité terrain de --from, recopiée pour chaque photo");
        println!();
        println!("Les documents sont fictifs : les IBAN sont structurellement valides");
        println!("mais tirés au hasard, et ne désignent aucun compte réel.");
        exit(0);
    }

    let Some(out) = value(&args, "--out").map(PathBuf::from) else {
        eprintln!("--out est obligatoire");
        exit(1);
    };

    let seed = value(&args, "--seed")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);

    let count = value(&args, "--count").and_then(|s| s.parse::<usize>().ok());

    if let Some(from) = value(&args, "--from").map(PathBuf::from) {
        let Some(truth) = value(&args, "--truth").map(PathBuf::from) else {
            eprintln!("--from demande --truth");
            exit(1);
        };

        match synth::derive::write(&from, &truth, &out, seed) {
            Ok(written) => println!("{} photos écrites dans {}", written, out.display()),
            Err(e) => {
                eprintln!("échec de la dérivation : {}", e);
                exit(1);
            }
        }
        return;
    }

    match synth::write(&out, seed, count) {
        Ok(written) => println!("{} documents écrits dans {}", written, out.display()),
        Err(e) => {
            eprintln!("échec de l'écriture du corpus : {}", e);
            exit(1);
        }
    }
}
