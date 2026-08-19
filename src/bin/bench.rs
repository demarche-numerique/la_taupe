//! Mesure la reconnaissance de RIB sur un corpus.
//!
//!     cargo run --release --features bench --bin bench -- \
//!       --corpus <dir> [--truth <csv>] [--confusion] [--jobs N]
//!
//! Le rapport ne contient aucun texte reconnu : un corpus de documents personnels peut
//! être mesuré sans que son contenu n'en ressorte.

use std::path::PathBuf;
use std::process::exit;

use la_taupe::bench::{self, truth::TruthSet};

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.contains(&String::from("--help")) || args.len() == 1 {
        println!("Mesure la reconnaissance de RIB sur un corpus.");
        println!();
        println!("  --corpus <dir>   dossier des documents (obligatoire)");
        println!("  --truth <csv>    vérité terrain, <corpus>/truth.csv par défaut");
        println!("  --confusion      ajoute la matrice de confusion caractère agrégée");
        println!("  --profile        ajoute le profil de forme du texte reconnu, par verdict");
        println!("  --jobs <n>       documents traités de front, 4 par défaut");
        println!("  --bootstrap      écrit une vérité terrain amorcée sur stdout,");
        println!("                   en ne retenant que les IBAN validant mod-97 et clé RIB");
        println!("  --check          contrôle la forme de la vérité terrain sans rien mesurer");
        println!();
        println!("Le rapport ne contient ni IBAN, ni nom, ni texte reconnu.");
        exit(0);
    }

    let Some(corpus) = value(&args, "--corpus").map(PathBuf::from) else {
        eprintln!("--corpus est obligatoire");
        exit(1);
    };

    let jobs = value(&args, "--jobs")
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(4);

    if args.contains(&String::from("--bootstrap")) {
        match bench::bootstrap(&corpus, jobs) {
            Ok(csv) => print!("{}", csv),
            Err(e) => {
                eprintln!("{}", e);
                exit(1);
            }
        }
        return;
    }

    let truth_path = value(&args, "--truth")
        .map(PathBuf::from)
        .unwrap_or_else(|| corpus.join("truth.csv"));

    let truths = match TruthSet::load(&truth_path) {
        Ok(truths) => truths,
        Err(e) => {
            eprintln!("{}", e);
            eprintln!("Amorcer un fichier avec --bootstrap, puis le corriger à la main.");
            exit(1);
        }
    };

    if truths.is_empty() {
        eprintln!("{} ne contient aucune ligne", truth_path.display());
        exit(1);
    }

    if args.contains(&String::from("--check")) {
        let checks = bench::check::check(&corpus, &truths);
        print!("{}", bench::check::render(&checks));
        return;
    }

    let with_confusion = args.contains(&String::from("--confusion"));

    match bench::run(&corpus, &truths, with_confusion, jobs) {
        Ok(report) => {
            print!("{}", report.render_files());
            print!("{}", report.render_summary());

            if with_confusion && !report.confusion.is_empty() {
                print!("{}", report.confusion.render());
            }

            if args.contains(&String::from("--profile")) {
                print!("{}", report.render_profiles());
            }
        }
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    }
}
