//! Vérité terrain d'un corpus, et comparaison avec ce que le pipeline a produit.
//!
//! Le fichier reste sur le poste qui héberge le corpus. Rien de ce qu'il contient ne
//! ressort du banc : il n'alimente que des verdicts.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::provenance::{BlockOutcome, HolderBlock, OcrLine};
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
    /// Rectangle du titulaire annoté, en fractions de la page : x0, y0, x1, y1.
    pub holder_box: Option<[f32; 4]>,
    /// Page de ce rectangle, 1 par défaut.
    pub holder_page: Option<u32>,
}

/// Où était le titulaire annoté, sur un document où il a été manqué : le sort du bloc
/// candidat qui le couvre, sans rien de son contenu. Dit si le bon bloc a été lu puis
/// écarté — et par quel filtre — ou s'il n'a jamais été regardé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDiagnosis {
    /// Le bloc qui couvre le titulaire a été retenu : c'est le bornage ou la lecture.
    Kept,
    /// Il a été vu, puis écarté ou supplanté.
    Rejected(BlockOutcome),
    /// Aucun bloc ne le couvre, parce que ses lignes ont été lues comme une seule,
    /// trop haute, écartée des ancres avec son code postal.
    MergedLines,
    /// Aucun bloc ne le couvre, et aucune de ses lignes n'a la forme d'un code postal :
    /// il n'y en a pas, ou il a été mal lu.
    PostalNotRead,
    /// Aucun bloc candidat ne le couvre, pour une autre raison.
    Missed,
    /// Comparaison impossible : chemin texte, autre page, place inconnue.
    NotComparable,
}

impl BlockDiagnosis {
    pub fn label(&self) -> String {
        match self {
            BlockDiagnosis::Kept => "bon bloc retenu (bornage ou lecture)".to_string(),
            BlockDiagnosis::Rejected(outcome) => format!("bon bloc {}", outcome.as_str()),
            BlockDiagnosis::MergedLines => {
                "titulaire dans une ligne OCR trop haute (écartée)".to_string()
            }
            BlockDiagnosis::PostalNotRead => "aucun code postal lu dans le titulaire".to_string(),
            BlockDiagnosis::Missed => "aucun bloc candidat sur le titulaire".to_string(),
            BlockDiagnosis::NotComparable => "non comparable".to_string(),
        }
    }
}

/// Part du rectangle annoté couverte par un bloc.
fn coverage(block: [f32; 4], truth: [f32; 4]) -> f32 {
    let w = block[2].min(truth[2]) - block[0].max(truth[0]);
    let h = block[3].min(truth[3]) - block[1].max(truth[1]);
    let area = (truth[2] - truth[0]) * (truth[3] - truth[1]);
    if w <= 0.0 || h <= 0.0 || area <= 0.0 {
        0.0
    } else {
        w * h / area
    }
}

/// Le bloc « bon » couvre au moins la moitié du rectangle annoté : les recadrages sont
/// plus larges que le bloc imprimé, et un bloc tronqué reste le bon bloc.
pub fn diagnose_blocks(
    truth_box: [f32; 4],
    truth_page: u32,
    page: Option<u32>,
    blocks: &[HolderBlock],
    lines: &[OcrLine],
) -> BlockDiagnosis {
    if page != Some(truth_page) {
        return BlockDiagnosis::NotComparable;
    }
    let placed: Vec<(f32, BlockOutcome)> = blocks
        .iter()
        .filter_map(|b| b.rect.map(|r| (coverage(r, truth_box), b.outcome)))
        .collect();
    if placed.is_empty() && !blocks.is_empty() {
        return BlockDiagnosis::NotComparable;
    }
    let good: Vec<&(f32, BlockOutcome)> = placed.iter().filter(|(c, _)| *c >= 0.5).collect();
    if good.iter().any(|(_, o)| *o == BlockOutcome::Kept) {
        return BlockDiagnosis::Kept;
    }
    if let Some((_, outcome)) = good.iter().max_by(|a, b| a.0.total_cmp(&b.0)) {
        return BlockDiagnosis::Rejected(*outcome);
    }

    // aucun bloc sur le titulaire : les lignes lues disent pourquoi
    let centred = |r: [f32; 4]| {
        let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
        cx >= truth_box[0] && cx <= truth_box[2] && cy >= truth_box[1] && cy <= truth_box[3]
    };
    let merged = lines
        .iter()
        .any(|l| l.oversized && l.rect.is_some_and(|r| coverage(r, truth_box) >= 0.5));
    let postal = lines
        .iter()
        .any(|l| l.postal && l.rect.is_some_and(centred));
    if merged {
        BlockDiagnosis::MergedLines
    } else if !lines.is_empty() && !postal {
        BlockDiagnosis::PostalNotRead
    } else {
        BlockDiagnosis::Missed
    }
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
    /// Format : `file;iban;bic;account_holder[;src;recipe;expect;page;box]`, les lignes
    /// du titulaire séparées par `|`, le rectangle du titulaire en fractions de la page
    /// (`x0,y0,x1,y1`). Les colonnes sont repérées par leur nom, donc leur ordre est libre
    /// et les dernières facultatives.
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
        let (page_column, box_column) = (column("page"), column("box"));

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
                    holder_box: text(box_column).and_then(|value| {
                        let v: Vec<f32> = value
                            .split(',')
                            .filter_map(|n| n.trim().parse().ok())
                            .collect();
                        (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
                    }),
                    holder_page: text(page_column).and_then(|value| value.parse().ok()),
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
            "file;iban;bic;account_holder;src;recipe;expect;page;box\n\
             a.pdf;FR7630001000644919009562088;SOGEFRPP;M MATISSE|44100 NANTES;pdf_text;natif;ok;1;0.1,0.2,0.4,0.3\n\
             b.pdf;;;;pdf_img;h20;known_failure;;\n",
        )
        .unwrap();

        let set = TruthSet::load(&path).unwrap();

        assert_eq!(set.len(), 2);

        let a = set.get("a.pdf").unwrap();
        assert_eq!(a.holder.as_deref(), Some("M MATISSE\n44100 NANTES"));
        assert!(!a.known_failure);
        assert_eq!(a.holder_box, Some([0.1, 0.2, 0.4, 0.3]));
        assert_eq!(a.holder_page, Some(1));

        let b = set.get("b.pdf").unwrap();
        assert_eq!(b.iban, None);
        assert!(b.known_failure);
        assert_eq!(b.holder_box, None);

        fs::remove_dir_all(&dir).ok();
    }

    /// Le bon bloc est celui qui couvre au moins la moitié du rectangle annoté ; on dit
    /// ce qu'il est devenu, ou qu'aucun candidat ne tombait dessus.
    #[test]
    fn the_block_covering_the_annotated_holder_is_diagnosed() {
        let truth = [0.5, 0.2, 0.8, 0.3];
        let at = |rect: [f32; 4], outcome| HolderBlock {
            rect: Some(rect),
            outcome,
        };
        let agency = at([0.1, 0.2, 0.4, 0.3], BlockOutcome::Domiciliation);
        let holder = at([0.45, 0.15, 0.85, 0.32], BlockOutcome::NotDesignated);

        assert_eq!(
            diagnose_blocks(truth, 1, Some(1), &[agency.clone(), holder.clone()], &[]),
            BlockDiagnosis::Rejected(BlockOutcome::NotDesignated)
        );
        assert_eq!(
            diagnose_blocks(truth, 1, Some(1), std::slice::from_ref(&agency), &[]),
            BlockDiagnosis::Missed
        );
        let kept = at([0.5, 0.2, 0.7, 0.3], BlockOutcome::Kept);
        assert_eq!(
            diagnose_blocks(truth, 1, Some(1), &[kept], &[]),
            BlockDiagnosis::Kept
        );
        // une autre page, ou le chemin texte (pas de page rastérisée) : rien à comparer
        assert_eq!(
            diagnose_blocks(truth, 2, Some(1), std::slice::from_ref(&holder), &[]),
            BlockDiagnosis::NotComparable
        );
        assert_eq!(
            diagnose_blocks(truth, 1, None, &[], &[]),
            BlockDiagnosis::NotComparable
        );
    }

    /// Sans bloc sur le titulaire, les lignes lues disent pourquoi : ses lignes lues
    /// comme une seule, trop haute ; ou pas de code postal parmi elles.
    #[test]
    fn the_read_lines_say_why_no_block_covers_the_holder() {
        let truth = [0.5, 0.2, 0.8, 0.3];
        let line = |rect: [f32; 4], postal, oversized| OcrLine {
            rect: Some(rect),
            postal,
            oversized,
        };

        let merged = line([0.5, 0.19, 0.8, 0.31], true, true);
        assert_eq!(
            diagnose_blocks(truth, 1, Some(1), &[], &[merged]),
            BlockDiagnosis::MergedLines
        );

        let name = line([0.5, 0.2, 0.7, 0.23], false, false);
        let street = line([0.5, 0.24, 0.7, 0.27], false, false);
        assert_eq!(
            diagnose_blocks(truth, 1, Some(1), &[], &[name.clone(), street.clone()]),
            BlockDiagnosis::PostalNotRead
        );

        let postal = line([0.5, 0.27, 0.7, 0.3], true, false);
        assert_eq!(
            diagnose_blocks(truth, 1, Some(1), &[], &[name, street, postal]),
            BlockDiagnosis::Missed
        );
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
