//! Non-régression sur corpus synthétique.
//!
//! Ce test valide la chaîne complète — génération, vérité terrain, mesure — sur des
//! documents fictifs, sans jamais toucher au corpus réel. Il est cantonné aux PDF
//! natifs pour rester exécutable en intégration continue : les variantes scannées
//! demandent tesseract et les modèles ocrs, absents de l'image de CI. Le volet OCR est
//! couvert par `full_corpus_meets_a_floor`, marqué `#[ignore]`.

use std::path::{Path, PathBuf};

use la_taupe::bench::{self, truth::TruthSet};
use la_taupe::synth;

/// Les cinq premiers documents de la grille sont les PDF natifs, un par gabarit.
const NATIVE_PDF_COUNT: usize = 5;

fn corpus_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("la_taupe_bench_{}", name));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn measure(dir: &Path, count: Option<usize>) -> bench::report::Report {
    synth::write(dir, 7, count).expect("écriture du corpus");

    let truths = TruthSet::load(&dir.join("truth.csv")).expect("vérité terrain");

    bench::run(dir, &truths, true, 4).expect("exécution du banc")
}

/// Sur PDF natif, aucune reconnaissance n'est en jeu : tout doit être exact. Un échec
/// ici signale une régression de `Rib::parse` ou un gabarit devenu irréaliste.
#[test]
fn native_pdfs_are_fully_recognised() {
    let dir = corpus_dir("native");
    let report = measure(&dir, Some(NATIVE_PDF_COUNT));

    assert_eq!(report.files.len(), NATIVE_PDF_COUNT);

    for file in &report.files {
        assert!(
            file.iban.is_ok(),
            "IBAN non reconnu sur {} (verdict {})",
            file.file,
            file.iban.as_str()
        );
        assert!(
            file.bic.is_ok(),
            "BIC non reconnu sur {} (verdict {})",
            file.file,
            file.bic.as_str()
        );
        assert!(
            file.holder_loose.is_ok(),
            "titulaire non reconnu sur {} (verdict {})",
            file.file,
            file.holder_loose.as_str()
        );
        assert_eq!(file.engine, Some("pdf_text"));
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// Le rapport ne doit jamais laisser fuiter le contenu des documents. On le vérifie sur
/// des données fictives, mais la propriété testée vaut pour le corpus réel.
#[test]
fn the_report_never_leaks_document_content() {
    let dir = corpus_dir("leak");
    let report = measure(&dir, Some(NATIVE_PDF_COUNT));

    let truths = TruthSet::load(&dir.join("truth.csv")).expect("vérité terrain");
    let rendered = format!("{}{}", report.render_files(), report.render_summary());

    for file in &report.files {
        let truth = truths.get(&file.file).expect("ligne de vérité");

        let iban = truth.iban.as_ref().expect("IBAN attendu");
        assert!(
            !rendered.contains(iban.as_str()),
            "l'IBAN de {} figure dans le rapport",
            file.file
        );

        for line in truth.holder.as_ref().expect("titulaire attendu").lines() {
            assert!(
                !rendered.contains(line),
                "le titulaire de {} figure dans le rapport",
                file.file
            );
        }
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// Mesure de bout en bout, OCR compris. Demande tesseract et les modèles ocrs :
///
///     cargo test --release --features bench --test bench_synth -- --ignored --nocapture
#[test]
#[ignore]
fn full_corpus_meets_a_floor() {
    let dir = corpus_dir("full");
    let report = measure(&dir, None);

    println!("{}", report.render_files());
    println!("{}", report.render_summary());

    let measurable: Vec<_> = report.files.iter().filter(|f| !f.known_failure).collect();
    let recognised = measurable.iter().filter(|f| f.iban.is_ok()).count();
    let rate = recognised as f32 / measurable.len() as f32;

    // plancher volontairement bas : il protège d'un effondrement, pas d'une variation
    assert!(
        rate >= 0.80,
        "taux de reconnaissance IBAN tombé à {:.1} %",
        rate * 100.0
    );

    std::fs::remove_dir_all(&dir).ok();
}
