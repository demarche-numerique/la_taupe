//! Vérité terrain d'un corpus, et comparaison avec ce que le pipeline a produit.
//!
//! Le fichier reste sur le poste qui héberge le corpus. Rien de ce qu'il contient ne
//! ressort du banc : il n'alimente que des verdicts.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::rib::normalize_iban;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Truth {
    pub iban: Option<String>,
    pub bic: Option<String>,
    pub holder: Option<String>,
    /// Cas dont on sait qu'ils échouent : comptabilisés à part, pour rester visibles
    /// sans peser sur le taux de réussite.
    pub known_failure: bool,
    pub src: Option<String>,
    pub recipe: Option<String>,
}

/// Nature d'un écart sur le titulaire, sans son contenu.
///
/// « Faux » ne dit pas comment : un bloc tronqué, un bloc débordant sur la
/// domiciliation, une civilité mal lue et une adresse déformée appellent des correctifs
/// différents. Ces catégories se déduisent de la seule comparaison des formes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolderMismatch {
    /// Moins de lignes que prévu : bloc coupé.
    Truncated,
    /// Plus de lignes que prévu : bloc débordant sur autre chose.
    Overflowing,
    /// Même nombre de lignes, nom juste, la ligne de ville (code postal) différente.
    CityLine,
    /// Même nombre de lignes, nom juste, une ligne de voie différente, ville juste.
    StreetLine,
    /// Même nombre de lignes, nom juste, plusieurs lignes d'adresse différentes.
    AddressOnly,
    /// Même nombre de lignes, première ligne différente d'un ou deux caractères.
    NameNearMiss,
    /// Même nombre de lignes, première ligne sans rapport.
    NameWrong,
}

impl HolderMismatch {
    pub fn as_str(&self) -> &'static str {
        match self {
            HolderMismatch::Truncated => "tronqué",
            HolderMismatch::Overflowing => "débordant",
            HolderMismatch::CityLine => "ville",
            HolderMismatch::StreetLine => "voie",
            HolderMismatch::AddressOnly => "adresse",
            HolderMismatch::NameNearMiss => "nom ±1",
            HolderMismatch::NameWrong => "nom faux",
        }
    }
}

/// Distance de Levenshtein, sur les caractères.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();

    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }

    prev[b.len()]
}

/// Catégorise l'écart entre titulaire attendu et trouvé, en comparaison souple.
pub fn classify_holder_mismatch(expected: &str, found: &str) -> HolderMismatch {
    let expected: Vec<String> = expected
        .lines()
        .map(normalize_holder_loose)
        .filter(|l| !l.is_empty())
        .collect();
    let found: Vec<String> = found
        .lines()
        .map(normalize_holder_loose)
        .filter(|l| !l.is_empty())
        .collect();

    if found.len() < expected.len() {
        return HolderMismatch::Truncated;
    }
    if found.len() > expected.len() {
        return HolderMismatch::Overflowing;
    }

    let (Some(e0), Some(f0)) = (expected.first(), found.first()) else {
        return HolderMismatch::NameWrong;
    };

    if e0 == f0 {
        // quelles lignes d'adresse diffèrent ? la ligne de ville porte un code postal
        let differing: Vec<usize> = (1..expected.len())
            .filter(|&i| expected[i] != found[i])
            .collect();
        let has_cp = |s: &str| {
            s.split_whitespace()
                .any(|w| w.len() == 5 && w.chars().all(|c| c.is_ascii_digit()))
        };
        return match differing.as_slice() {
            [i] if has_cp(&expected[*i]) => HolderMismatch::CityLine,
            [_] => HolderMismatch::StreetLine,
            _ => HolderMismatch::AddressOnly,
        };
    }

    if edit_distance(e0, f0) <= 2 {
        HolderMismatch::NameNearMiss
    } else {
        HolderMismatch::NameWrong
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Attendu et trouvé, identiques.
    Ok,
    /// Attendu et trouvé, mais différents.
    Ko,
    /// Attendu, rien trouvé.
    NotFound,
    /// Rien d'attendu : hors du calcul des taux.
    NoTruth,
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Ok => "OK",
            Verdict::Ko => "KO",
            Verdict::NotFound => "--",
            Verdict::NoTruth => "?",
        }
    }

    /// Seuls les cas où une vérité est renseignée entrent dans le taux.
    pub fn counts(&self) -> bool {
        !matches!(self, Verdict::NoTruth)
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Verdict::Ok)
    }
}

fn strip_accents(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'E',
            'à' | 'â' | 'ä' | 'À' | 'Â' | 'Ä' => 'A',
            'î' | 'ï' | 'Î' | 'Ï' => 'I',
            'ô' | 'ö' | 'Ô' | 'Ö' => 'O',
            'ù' | 'û' | 'ü' | 'Ù' | 'Û' | 'Ü' => 'U',
            'ç' | 'Ç' => 'C',
            c => c,
        })
        .collect()
}

pub fn normalize_bic(bic: &str) -> String {
    bic.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Comparaison stricte du titulaire : lignes conservées, espaces de bord retirés.
pub fn normalize_holder_strict(holder: &str) -> String {
    holder
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Comparaison souple : casse, accents, ponctuation et découpage en lignes ignorés.
/// Un titulaire correctement lu mais autrement découpé reste un succès.
pub fn normalize_holder_loose(holder: &str) -> String {
    strip_accents(holder)
        .to_uppercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<&str>>()
        .join(" ")
}

/// Comparaison par contenu : tous les espaces retirés. Un titulaire lu
/// « 44800STHERBLAIN » porte la même information que « 44800 ST HERBLAIN » — un moteur
/// qui colle les mots n'a pas mal lu, il a mal segmenté. Les deux verdicts sont rendus :
/// celui-ci dit si le contenu est là, le souple dit si le rendu est exploitable tel quel.
pub fn normalize_holder_content(holder: &str) -> String {
    normalize_holder_loose(holder).replace(' ', "")
}

fn compare(
    expected: Option<&String>,
    found: Option<String>,
    normalize: fn(&str) -> String,
) -> Verdict {
    match (expected, found) {
        (None, _) => Verdict::NoTruth,
        (Some(expected), _) if expected.trim().is_empty() => Verdict::NoTruth,
        (Some(_), None) => Verdict::NotFound,
        (Some(expected), Some(found)) => {
            if normalize(expected) == normalize(&found) {
                Verdict::Ok
            } else {
                Verdict::Ko
            }
        }
    }
}

impl Truth {
    pub fn iban_verdict(&self, found: Option<&str>) -> Verdict {
        compare(
            self.iban.as_ref(),
            found.map(|s| s.to_string()),
            normalize_iban,
        )
    }

    pub fn bic_verdict(&self, found: Option<&str>) -> Verdict {
        compare(
            self.bic.as_ref(),
            found.map(|s| s.to_string()),
            normalize_bic,
        )
    }

    pub fn holder_strict_verdict(&self, found: Option<&str>) -> Verdict {
        compare(
            self.holder.as_ref(),
            found.map(|s| s.to_string()),
            normalize_holder_strict,
        )
    }

    pub fn holder_loose_verdict(&self, found: Option<&str>) -> Verdict {
        compare(
            self.holder.as_ref(),
            found.map(|s| s.to_string()),
            normalize_holder_loose,
        )
    }

    pub fn holder_content_verdict(&self, found: Option<&str>) -> Verdict {
        compare(
            self.holder.as_ref(),
            found.map(|s| s.to_string()),
            normalize_holder_content,
        )
    }
}

#[derive(Default)]
pub struct TruthSet(HashMap<String, Truth>);

impl TruthSet {
    /// Format : `file;iban;bic;account_holder[;src;recipe;expect]`, les lignes du
    /// titulaire séparées par `|`. Les colonnes sont repérées par leur nom, donc leur
    /// ordre est libre et les trois dernières facultatives.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = fs::read_to_string(path)
            .map_err(|e| format!("lecture de {} : {}", path.display(), e))?;

        let mut lines = content.lines();

        let header: Vec<&str> = lines
            .next()
            .ok_or_else(|| format!("{} est vide", path.display()))?
            .split(';')
            .map(|field| field.trim())
            .collect();

        let column = |name: &str| header.iter().position(|field| *field == name);

        let file_column = column("file")
            .ok_or_else(|| format!("{} n'a pas de colonne `file`", path.display()))?;

        let (iban_column, bic_column) = (column("iban"), column("bic"));
        let (holder_column, expect_column) = (column("account_holder"), column("expect"));
        let (src_column, recipe_column) = (column("src"), column("recipe"));

        let mut entries = HashMap::new();

        for line in lines.filter(|line| !line.trim().is_empty()) {
            let fields: Vec<&str> = line.split(';').collect();

            let Some(file) = fields.get(file_column) else {
                continue;
            };

            let text = |index: Option<usize>| {
                index
                    .and_then(|i| fields.get(i))
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
            };

            entries.insert(
                file.trim().to_string(),
                Truth {
                    iban: text(iban_column),
                    bic: text(bic_column),
                    holder: text(holder_column).map(|value| value.replace('|', "\n")),
                    known_failure: text(expect_column).as_deref() == Some("known_failure"),
                    src: text(src_column),
                    recipe: text(recipe_column),
                },
            );
        }

        Ok(TruthSet(entries))
    }

    pub fn get(&self, file: &str) -> Option<&Truth> {
        self.0.get(file)
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &Truth)> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn truth() -> Truth {
        Truth {
            iban: Some("FR7630001000644919009562088".to_string()),
            bic: Some("SOGEFRPP".to_string()),
            holder: Some("M MATISSE HENRI\n51 RUE BERNARD ROY".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn iban_comparison_ignores_grouping() {
        assert_eq!(
            truth().iban_verdict(Some("FR76 3000 1000 6449 1900 9562 088")),
            Verdict::Ok
        );
        assert_eq!(
            truth().iban_verdict(Some("FR7630001000644919009562087")),
            Verdict::Ko
        );
        assert_eq!(truth().iban_verdict(None), Verdict::NotFound);
    }

    /// La fixture `rib_bourso` attend un BIC espacé : la comparaison doit l'accepter.
    #[test]
    fn bic_comparison_ignores_spacing() {
        let truth = Truth {
            bic: Some("BOUSFRPPXXX".to_string()),
            ..Default::default()
        };

        assert_eq!(truth.bic_verdict(Some("BOUS FRPP XXX")), Verdict::Ok);
    }

    #[test]
    fn holder_mismatches_are_categorised_by_shape() {
        let expected = "M MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES";

        assert_eq!(
            classify_holder_mismatch(expected, "M MATISSE HENRI"),
            HolderMismatch::Truncated
        );
        assert_eq!(
            classify_holder_mismatch(
                expected,
                "M MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES\nDOMICILIATION"
            ),
            HolderMismatch::Overflowing
        );
        assert_eq!(
            classify_holder_mismatch(
                expected,
                "M MATISSE HENRI\n51 RUE BERNARD R0Y\n44100 NANTES"
            ),
            HolderMismatch::StreetLine
        );
        assert_eq!(
            classify_holder_mismatch(
                expected,
                "M MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTE5"
            ),
            HolderMismatch::CityLine
        );
        assert_eq!(
            classify_holder_mismatch(
                expected,
                "M MATISSE HENRI\n51 RUE BERNARD R0Y\n44100 NANTE5"
            ),
            HolderMismatch::AddressOnly
        );
        assert_eq!(
            classify_holder_mismatch(expected, "M MATISSE HENR\n51 RUE BERNARD ROY\n44100 NANTES"),
            HolderMismatch::NameNearMiss
        );
        assert_eq!(
            classify_holder_mismatch(
                expected,
                "AGENCE DE NANTES\n51 RUE BERNARD ROY\n44100 NANTES"
            ),
            HolderMismatch::NameWrong
        );
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        assert_eq!(edit_distance("HENRI", "HENR"), 1);
        assert_eq!(edit_distance("VICTOR", "VIGTOR"), 1);
        assert_eq!(edit_distance("SUZANNE", "SIIZANNE"), 2);
        assert_eq!(edit_distance("MATISSE", "AGENCE"), 5);
    }

    #[test]
    fn loose_holder_tolerates_case_accents_and_line_breaks() {
        let truth = truth();

        assert_eq!(
            truth.holder_strict_verdict(Some("M MATISSE HENRI 51 RUE BERNARD ROY")),
            Verdict::Ko
        );
        assert_eq!(
            truth.holder_loose_verdict(Some("m matisse henri 51 rue bernard roy")),
            Verdict::Ok
        );
        assert_eq!(
            truth.holder_loose_verdict(Some("M MATISSE HENRI")),
            Verdict::Ko
        );
    }

    /// Les RIB intercalent souvent une ligne vide entre le nom et l'adresse. Elle est
    /// ignorée de part et d'autre : inutile de la représenter dans la vérité terrain.
    #[test]
    fn blank_lines_are_ignored_on_both_sides() {
        let truth = Truth {
            holder: Some(
                "Prenom1 Nom1 ou Prenom2 Nom2\n234 Rue des exemples\n44300 Nantes".to_string(),
            ),
            ..Default::default()
        };

        let with_blank = "Prenom1 Nom1 ou Prenom2 Nom2\n\n234 Rue des exemples\n44300 Nantes";

        assert_eq!(truth.holder_strict_verdict(Some(with_blank)), Verdict::Ok);
        assert_eq!(truth.holder_loose_verdict(Some(with_blank)), Verdict::Ok);
    }

    /// Les mots collés par un moteur ne sont pas une erreur de lecture.
    #[test]
    fn glued_words_still_match_by_content() {
        let truth = Truth {
            holder: Some("M MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES".to_string()),
            ..Default::default()
        };
        let glued = "M MATISSE HENRI\n51RUE BERNARD ROY\n44100NANTES";

        assert_eq!(truth.holder_loose_verdict(Some(glued)), Verdict::Ko);
        assert_eq!(truth.holder_content_verdict(Some(glued)), Verdict::Ok);

        // un vrai écart reste un écart
        assert_eq!(
            truth.holder_content_verdict(Some("M MATISSE HENR\n51 RUE BERNARD ROY\n44100 NANTES")),
            Verdict::Ko
        );
    }

    /// Sans vérité renseignée, le champ sort du calcul du taux plutôt que de compter
    /// comme un échec.
    #[test]
    fn missing_truth_is_excluded_from_scoring() {
        let empty = Truth::default();

        assert_eq!(empty.iban_verdict(Some("FR76...")), Verdict::NoTruth);
        assert!(!Verdict::NoTruth.counts());
        assert!(Verdict::NotFound.counts());
    }

    #[test]
    fn loads_a_csv_with_optional_columns() {
        let dir = std::env::temp_dir().join("la_taupe_truth_test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("truth.csv");

        fs::write(
            &path,
            "file;iban;bic;account_holder;src;recipe;expect\n\
             a.pdf;FR7630001000644919009562088;SOGEFRPP;M MATISSE|44100 NANTES;pdf_text;natif;ok\n\
             b.pdf;;;;pdf_img;h20;known_failure\n",
        )
        .unwrap();

        let set = TruthSet::load(&path).unwrap();

        assert_eq!(set.len(), 2);

        let a = set.get("a.pdf").unwrap();
        assert_eq!(a.holder.as_deref(), Some("M MATISSE\n44100 NANTES"));
        assert!(!a.known_failure);

        let b = set.get("b.pdf").unwrap();
        assert_eq!(b.iban, None);
        assert!(b.known_failure);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_csv_without_file_column_is_rejected() {
        let dir = std::env::temp_dir().join("la_taupe_truth_bad");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("truth.csv");

        fs::write(&path, "iban;bic\nFR76;SOGEFRPP\n").unwrap();

        assert!(TruthSet::load(&path).is_err());

        fs::remove_dir_all(&dir).ok();
    }
}
