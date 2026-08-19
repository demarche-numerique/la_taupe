//! Contrôle de forme d'une vérité terrain.
//!
//! Saisir une vérité terrain à la main est fastidieux et silencieusement faillible : un
//! chiffre d'IBAN inversé, un séparateur oublié, un nom de fichier approximatif, et le
//! banc mesure contre une référence fausse sans jamais s'en plaindre.
//!
//! Ce contrôle exploite la redondance de l'IBAN pour attraper ces erreurs, et ne rend
//! que des verdicts : il peut être lancé sur un corpus personnel et son résultat
//! transmis tel quel.

use std::path::Path;

use regex::Regex;

use crate::rib::{check_rib_key, normalize_iban};

use super::truth::{normalize_bic, TruthSet};

#[derive(Debug, PartialEq, Eq)]
pub enum IbanCheck {
    Missing,
    /// Un IBAN français fait 27 caractères.
    BadLength(usize),
    NotFrench,
    /// Échoue la clé de contrôle internationale : au moins un caractère est faux.
    BadChecksum,
    /// Passe le mod-97 mais pas la clé RIB nationale : deux erreurs se compensent,
    /// ou la clé a été recopiée de travers.
    BadRibKey,
    Ok,
}

impl IbanCheck {
    pub fn as_str(&self) -> &'static str {
        match self {
            IbanCheck::Missing => "-",
            IbanCheck::BadLength(_) => "longueur",
            IbanCheck::NotFrench => "pas FR",
            IbanCheck::BadChecksum => "mod-97 KO",
            IbanCheck::BadRibKey => "clé RIB KO",
            IbanCheck::Ok => "OK",
        }
    }

    pub fn is_problem(&self) -> bool {
        !matches!(self, IbanCheck::Ok | IbanCheck::Missing)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BicCheck {
    Missing,
    BadForm,
    Ok,
}

impl BicCheck {
    pub fn as_str(&self) -> &'static str {
        match self {
            BicCheck::Missing => "-",
            BicCheck::BadForm => "forme",
            BicCheck::Ok => "OK",
        }
    }

    pub fn is_problem(&self) -> bool {
        matches!(self, BicCheck::BadForm)
    }
}

pub struct LineCheck {
    pub file: String,
    /// Faux si aucun document de ce nom n'existe dans le corpus.
    pub present: bool,
    pub iban: IbanCheck,
    pub bic: BicCheck,
    pub holder_lines: usize,
    /// Un code postal au-delà de la première ligne suggère une adresse correctement
    /// découpée ; sur la première ligne seule, c'est un séparateur oublié.
    pub holder_has_postal_code: bool,
}

impl LineCheck {
    /// Adresse vraisemblablement collée au nom, faute de séparateur `|`.
    pub fn holder_looks_unsplit(&self) -> bool {
        self.holder_lines == 1 && self.holder_has_postal_code
    }

    pub fn has_problem(&self) -> bool {
        !self.present
            || self.iban.is_problem()
            || self.bic.is_problem()
            || self.holder_looks_unsplit()
    }
}

fn check_iban(iban: Option<&String>) -> IbanCheck {
    let Some(iban) = iban else {
        return IbanCheck::Missing;
    };

    let normalized = normalize_iban(iban);

    if !normalized.starts_with("FR") {
        return IbanCheck::NotFrench;
    }

    if normalized.len() != 27 {
        return IbanCheck::BadLength(normalized.len());
    }

    if normalized.parse::<iban::Iban>().is_err() {
        return IbanCheck::BadChecksum;
    }

    if !check_rib_key(&normalized) {
        return IbanCheck::BadRibKey;
    }

    IbanCheck::Ok
}

fn check_bic(bic: Option<&String>) -> BicCheck {
    let Some(bic) = bic else {
        return BicCheck::Missing;
    };

    let normalized = normalize_bic(bic);

    // 4 lettres d'établissement, 2 de pays, 2 alphanumériques, et 3 en option
    let form = Regex::new(r"^[A-Z]{4}[A-Z]{2}[A-Z0-9]{2}([A-Z0-9]{3})?$").unwrap();

    if form.is_match(&normalized) {
        BicCheck::Ok
    } else {
        BicCheck::BadForm
    }
}

pub fn check(corpus: &Path, truths: &TruthSet) -> Vec<LineCheck> {
    let postal_code = Regex::new(r"\b\d{5}\b").unwrap();

    let mut checks: Vec<LineCheck> = truths
        .entries()
        .map(|(file, truth)| {
            let holder_lines: Vec<&str> = truth
                .holder
                .as_deref()
                .map(|holder| holder.lines().filter(|l| !l.trim().is_empty()).collect())
                .unwrap_or_default();

            LineCheck {
                file: file.clone(),
                present: corpus.join(file).is_file(),
                iban: check_iban(truth.iban.as_ref()),
                bic: check_bic(truth.bic.as_ref()),
                holder_lines: holder_lines.len(),
                holder_has_postal_code: holder_lines.iter().any(|line| postal_code.is_match(line)),
            }
        })
        .collect();

    checks.sort_by(|a, b| a.file.cmp(&b.file));

    checks
}

/// Rendu ne portant que des verdicts, des comptes et des noms de fichiers.
pub fn render(checks: &[LineCheck]) -> String {
    let mut out = format!(
        "{:<28} {:<8} {:<12} {:<8} {:<12}\n",
        "file", "présent", "iban", "bic", "titulaire"
    );

    for check in checks {
        let holder = match check.holder_lines {
            0 => "-".to_string(),
            1 if check.holder_has_postal_code => "1 ligne (?)".to_string(),
            n => format!("{} lignes", n),
        };

        out.push_str(&format!(
            "{:<28} {:<8} {:<12} {:<8} {:<12}\n",
            check.file,
            if check.present { "oui" } else { "NON" },
            check.iban.as_str(),
            check.bic.as_str(),
            holder
        ));
    }

    let filled = checks
        .iter()
        .filter(|c| c.iban != IbanCheck::Missing)
        .count();
    let problems: Vec<&LineCheck> = checks.iter().filter(|c| c.has_problem()).collect();

    out.push_str(&format!(
        "\n{} ligne(s) sur {} portent un IBAN\n",
        filled,
        checks.len()
    ));

    if problems.is_empty() {
        out.push_str("Aucune anomalie de forme.\n");
        return out;
    }

    out.push_str(&format!("\n{} anomalie(s) :\n", problems.len()));

    for problem in problems {
        if !problem.present {
            out.push_str(&format!(
                "  {} : aucun document de ce nom dans le corpus\n",
                problem.file
            ));
        }
        if problem.iban.is_problem() {
            out.push_str(&format!(
                "  {} : IBAN {} — vérifier la recopie\n",
                problem.file,
                problem.iban.as_str()
            ));
        }
        if problem.bic.is_problem() {
            out.push_str(&format!("  {} : BIC de forme inattendue\n", problem.file));
        }
        if problem.holder_looks_unsplit() {
            out.push_str(&format!(
                "  {} : titulaire sur une seule ligne alors qu'il porte un code postal —\n           \
                 séparateur `|` probablement oublié entre le nom et l'adresse\n",
                problem.file
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    static IBAN: &str = "FR7630001000644919009562088";

    #[test]
    fn a_well_formed_iban_passes_both_checksums() {
        assert_eq!(check_iban(Some(&IBAN.to_string())), IbanCheck::Ok);
        assert_eq!(
            check_iban(Some(&"FR76 3000 1000 6449 1900 9562 088".to_string())),
            IbanCheck::Ok
        );
        assert_eq!(check_iban(None), IbanCheck::Missing);
    }

    /// Le cas que le contrôle existe pour attraper : deux chiffres intervertis à la
    /// saisie. La clé internationale suffit à le voir.
    #[test]
    fn a_transcription_error_is_caught() {
        assert_eq!(
            check_iban(Some(&"FR7630001000644919009562880".to_string())),
            IbanCheck::BadChecksum
        );
        assert_eq!(
            check_iban(Some(&"FR763000100064491900956208".to_string())),
            IbanCheck::BadLength(26)
        );
        assert_eq!(
            check_iban(Some(&"DE89370400440532013000".to_string())),
            IbanCheck::NotFrench
        );
    }

    #[test]
    fn bic_form_is_validated_but_not_invented() {
        assert_eq!(check_bic(Some(&"SOGEFRPP".to_string())), BicCheck::Ok);
        assert_eq!(check_bic(Some(&"BOUS FRPP XXX".to_string())), BicCheck::Ok);
        assert_eq!(check_bic(Some(&"SOGE".to_string())), BicCheck::BadForm);
        assert_eq!(check_bic(None), BicCheck::Missing);
    }

    #[test]
    fn a_holder_glued_to_its_address_is_flagged() {
        let unsplit = LineCheck {
            file: "1.jpeg".to_string(),
            present: true,
            iban: IbanCheck::Ok,
            bic: BicCheck::Ok,
            holder_lines: 1,
            holder_has_postal_code: true,
        };

        assert!(unsplit.holder_looks_unsplit());
        assert!(unsplit.has_problem());

        let split = LineCheck {
            holder_lines: 3,
            ..unsplit
        };

        assert!(!split.holder_looks_unsplit());
        assert!(!split.has_problem());
    }

    /// Un titulaire d'une seule ligne sans adresse est parfaitement normal.
    #[test]
    fn a_name_without_address_is_not_flagged() {
        let name_only = LineCheck {
            file: "2.jpg".to_string(),
            present: true,
            iban: IbanCheck::Ok,
            bic: BicCheck::Ok,
            holder_lines: 1,
            holder_has_postal_code: false,
        };

        assert!(!name_only.holder_looks_unsplit());
        assert!(!name_only.has_problem());
    }
}
