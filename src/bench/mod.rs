//! Banc de mesure de la reconnaissance de RIB.
//!
//! Exécute le pipeline de production sur un corpus, confronte le résultat à une vérité
//! terrain qui ne quitte jamais la machine, et n'émet que des verdicts et des grandeurs
//! géométriques. Un corpus de documents personnels peut donc être mesuré sans que rien
//! de leur contenu ne figure dans le rapport.

pub mod check;
pub mod profile;
pub mod report;
pub mod truth;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use crate::analysis::vec_to_rib_traced;
use crate::provenance::Provenance;
use crate::rib::{check_rib_key, normalize_iban, Rib};

use report::{Confusion, Failure, FileReport, Report};
use truth::{TruthSet, Verdict};

/// Documents du corpus, triés pour que deux exécutions soient comparables ligne à ligne.
pub fn corpus_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("lecture de {} : {}", dir.display(), e))?;

    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name != "truth.csv" && !name.starts_with('.'))
                .unwrap_or(false)
        })
        .collect();

    files.sort();

    Ok(files)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("inconnu")
        .to_string()
}

fn analyse(path: &Path) -> (Option<Rib>, Provenance, Option<Failure>, u64) {
    let mut provenance = Provenance::default();

    let Ok(content) = fs::read(path) else {
        return (None, provenance, Some(Failure::Unreadable), 0);
    };

    crate::timing::reset();
    let started = Instant::now();
    let outcome = vec_to_rib_traced(content, &file_name(path), &mut provenance);
    let duration = started.elapsed().as_millis() as u64;
    provenance.timings = crate::timing::snapshot();

    match outcome {
        Ok(rib) => (rib, provenance, None, duration),
        Err(_) => (None, provenance, Some(Failure::UnsupportedType), duration),
    }
}

/// Isole chaque document : un `unwrap` malheureux sur un cas particulier ne doit pas
/// emporter la mesure des autres, surtout sur un corpus qu'on ne peut pas inspecter.
fn measure(path: &Path, truths: &TruthSet, confusion: Option<&Mutex<Confusion>>) -> FileReport {
    let measured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        measure_unguarded(path, truths, confusion)
    }));

    measured.unwrap_or_else(|_| {
        let mut file = FileReport::from_provenance(file_name(path), &Provenance::default());
        file.failure = Some(Failure::Panicked);
        file
    })
}

fn measure_unguarded(
    path: &Path,
    truths: &TruthSet,
    confusion: Option<&Mutex<Confusion>>,
) -> FileReport {
    let (rib, provenance, failure, duration) = analyse(path);
    let name = file_name(path);

    let mut file = FileReport::from_provenance(name.clone(), &provenance);
    file.duration_ms = duration;
    file.failure = failure;

    let Some(truth) = truths.get(&name) else {
        return file;
    };

    let found_iban = rib.as_ref().map(|r| r.iban());

    file.iban = truth.iban_verdict(found_iban);
    file.bic = truth.bic_verdict(rib.as_ref().and_then(|r| r.bic()));
    file.holder_strict = truth.holder_strict_verdict(rib.as_ref().and_then(|r| r.account_holder()));
    file.holder_loose = truth.holder_loose_verdict(rib.as_ref().and_then(|r| r.account_holder()));
    file.holder_content =
        truth.holder_content_verdict(rib.as_ref().and_then(|r| r.account_holder()));

    if file.holder_loose == Verdict::Ko {
        if let (Some(expected), Some(found)) = (
            truth.holder.as_ref(),
            rib.as_ref().and_then(|r| r.account_holder()),
        ) {
            file.holder_mismatch = Some(truth::classify_holder_mismatch(expected, found));
        }
    }
    file.known_failure = truth.known_failure;
    file.src = truth.src.clone();
    file.recipe = truth.recipe.clone();

    if file.iban == Verdict::Ko {
        if let (Some(confusion), Some(expected), Some(found)) =
            (confusion, truth.iban.as_ref(), found_iban)
        {
            confusion
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .observe(&normalize_iban(expected), &normalize_iban(found));
        }
    }

    file
}

/// Mesure le corpus. `jobs` fixe le nombre de documents traités de front.
pub fn run(
    dir: &Path,
    truths: &TruthSet,
    with_confusion: bool,
    jobs: usize,
) -> Result<Report, String> {
    let files = corpus_files(dir)?;
    let confusion = Mutex::new(Confusion::default());
    let jobs = jobs.max(1).min(files.len().max(1));

    let confusion_ref = if with_confusion {
        Some(&confusion)
    } else {
        None
    };

    let results: Vec<Vec<(usize, FileReport)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..jobs)
            .map(|job| {
                let files = &files;
                let truths = &truths;

                scope.spawn(move || {
                    files
                        .iter()
                        .enumerate()
                        .skip(job)
                        .step_by(jobs)
                        .map(|(index, path)| (index, measure(path, truths, confusion_ref)))
                        .collect::<Vec<(usize, FileReport)>>()
                })
            })
            .collect();

        handles
            .into_iter()
            .map(|handle| handle.join().expect("thread de mesure"))
            .collect()
    });

    let mut indexed: Vec<(usize, FileReport)> = results.into_iter().flatten().collect();
    indexed.sort_by_key(|(index, _)| *index);

    Ok(Report {
        files: indexed.into_iter().map(|(_, file)| file).collect(),
        confusion: confusion
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    })
}

/// Pré-remplit une vérité terrain à partir de ce que le pipeline trouve, en ne retenant
/// que les IBAN qui valident à la fois le mod-97 et la clé RIB.
///
/// Sans ce filtre, les erreurs actuelles se figeraient en référence et le banc
/// mesurerait sa propre complaisance.
pub fn bootstrap(dir: &Path, jobs: usize) -> Result<String, String> {
    let files = corpus_files(dir)?;
    let jobs = jobs.max(1).min(files.len().max(1));

    let rows: Vec<Vec<(usize, String)>> =
        std::thread::scope(|scope| {
            let handles: Vec<_> =
                (0..jobs)
                    .map(|job| {
                        let files = &files;

                        scope.spawn(move || {
                            files
                                .iter()
                                .enumerate()
                                .skip(job)
                                .step_by(jobs)
                                .map(|(index, path)| {
                                    let (rib, _, _, _) = std::panic::catch_unwind(
                                        std::panic::AssertUnwindSafe(|| analyse(path)),
                                    )
                                    .unwrap_or((None, Provenance::default(), None, 0));

                                    let trusted = rib.as_ref().filter(|rib| {
                                        let iban = normalize_iban(rib.iban());
                                        iban.parse::<iban::Iban>().is_ok() && check_rib_key(&iban)
                                    });

                                    let row = format!(
                                        "{};{};{};\n",
                                        file_name(path),
                                        trusted
                                            .map(|r| normalize_iban(r.iban()))
                                            .unwrap_or_default(),
                                        trusted
                                            .and_then(|r| r.bic())
                                            .map(truth::normalize_bic)
                                            .unwrap_or_default(),
                                    );

                                    (index, row)
                                })
                                .collect::<Vec<(usize, String)>>()
                        })
                    })
                    .collect();

            handles
                .into_iter()
                .map(|handle| handle.join().expect("thread d'amorçage"))
                .collect()
        });

    let mut indexed: Vec<(usize, String)> = rows.into_iter().flatten().collect();
    indexed.sort_by_key(|(index, _)| *index);

    let mut out = String::from("file;iban;bic;account_holder\n");
    for (_, row) in indexed {
        out.push_str(&row);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_listing_excludes_the_truth_file() {
        let dir = std::env::temp_dir().join("la_taupe_corpus_listing");
        fs::create_dir_all(&dir).unwrap();

        fs::write(dir.join("truth.csv"), "file\n").unwrap();
        fs::write(dir.join("b.pdf"), "x").unwrap();
        fs::write(dir.join("a.pdf"), "x").unwrap();
        fs::write(dir.join(".hidden"), "x").unwrap();

        let files = corpus_files(&dir).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(file_name(&files[0]), "a.pdf");
        assert_eq!(file_name(&files[1]), "b.pdf");

        fs::remove_dir_all(&dir).ok();
    }
}
