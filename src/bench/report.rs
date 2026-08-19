//! Rapport du banc.
//!
//! Garantie de confidentialité par le typage : hormis le nom du fichier et les
//! métadonnées reprises de la vérité terrain, un `FileReport` ne contient que des
//! nombres et des `&'static str`. Aucun texte reconnu ne peut donc y transiter, même
//! par inadvertance — un futur champ dynamique ne compilerait pas sans qu'on l'ajoute
//! explicitement ici.

use std::collections::BTreeMap;

use crate::provenance::{Provenance, TextStats};

use super::profile::{grouped, ProfileSummary};
use super::truth::{HolderMismatch, Verdict};

/// Motif d'abandon, catégorisé plutôt que recopié : un message d'erreur brut pourrait
/// contenir un fragment du document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    UnsupportedType,
    Unreadable,
    /// Le pipeline a paniqué sur ce document. Consigné comme un échec parmi d'autres :
    /// une campagne sur corpus réel ne doit pas être perdue pour un cas pathologique.
    Panicked,
}

impl Failure {
    pub fn as_str(&self) -> &'static str {
        match self {
            Failure::UnsupportedType => "type_non_supporte",
            Failure::Unreadable => "illisible",
            Failure::Panicked => "PANIQUE",
        }
    }
}

pub struct FileReport {
    pub file: String,
    pub iban: Verdict,
    pub bic: Verdict,
    pub holder_strict: Verdict,
    pub holder_loose: Verdict,
    /// Contenu identique une fois les espaces retirés : lu, même si mal segmenté.
    pub holder_content: Verdict,
    pub known_failure: bool,
    /// Forme attendue, reprise de la vérité terrain.
    pub src: Option<String>,
    /// Recette de dégradation, reprise de la vérité terrain.
    pub recipe: Option<String>,
    pub route: Option<&'static str>,
    pub engine: Option<&'static str>,
    pub anchor: Option<&'static str>,
    pub anchor_height: Option<u32>,
    pub angle_deg: Option<f32>,
    pub second_pass: bool,
    pub postal_anchors: u32,
    pub holder_candidates: u32,
    pub holder_blocks_read: u32,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub ocr_ms: u64,
    pub ocr_calls: u32,
    pub tess_ms: u64,
    pub tess_calls: u32,
    pub preprocess_ms: u64,
    pub poppler_ms: u64,
    pub failure: Option<Failure>,
    /// Comptages de forme du texte reconnu — jamais le texte.
    pub text_stats: Option<TextStats>,
    /// Nature de l'écart quand le titulaire est faux.
    pub holder_mismatch: Option<HolderMismatch>,
}

impl FileReport {
    pub fn from_provenance(file: String, provenance: &Provenance) -> Self {
        FileReport {
            file,
            iban: Verdict::NoTruth,
            bic: Verdict::NoTruth,
            holder_strict: Verdict::NoTruth,
            holder_loose: Verdict::NoTruth,
            holder_content: Verdict::NoTruth,
            known_failure: false,
            src: None,
            recipe: None,
            route: provenance.route.map(|r| r.as_str()),
            engine: provenance.engine.map(|e| e.as_str()),
            anchor: provenance.anchor.map(|a| a.as_str()),
            anchor_height: provenance.anchor_height,
            angle_deg: provenance.angle_deg,
            second_pass: provenance.second_pass,
            postal_anchors: provenance.postal_anchors,
            holder_candidates: provenance.holder_candidates,
            holder_blocks_read: provenance.holder_blocks_read,
            width: provenance.image_width,
            height: provenance.image_height,
            duration_ms: 0,
            ocr_ms: provenance.timings.ocr.as_millis() as u64,
            ocr_calls: provenance.timings.ocr_calls,
            tess_ms: provenance.timings.tesseract.as_millis() as u64,
            tess_calls: provenance.timings.tesseract_calls,
            preprocess_ms: provenance.timings.preprocess.as_millis() as u64,
            poppler_ms: provenance.timings.poppler.as_millis() as u64,
            failure: None,
            text_stats: provenance.page_text_stats.clone(),
            holder_mismatch: None,
        }
    }
}

fn optional<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|v| v.to_string())
        .unwrap_or_else(|| "-".to_string())
}

/// Matrice de confusion caractère et distribution des positions d'erreur, agrégées sur
/// l'ensemble du corpus.
///
/// Les deux sont émises séparément et ne sont jamais jointes : un couple
/// (position, caractère) sur un corpus réduit permettrait de reconstituer des fragments
/// d'IBAN, ce que des agrégats séparés interdisent.
#[derive(Default)]
pub struct Confusion {
    pairs: BTreeMap<(char, char), usize>,
    positions: BTreeMap<usize, usize>,
    length_mismatches: usize,
}

impl Confusion {
    pub fn observe(&mut self, expected: &str, found: &str) {
        if expected.len() != found.len() {
            self.length_mismatches += 1;
            return;
        }

        for (position, (a, b)) in expected.chars().zip(found.chars()).enumerate() {
            if a != b {
                *self.pairs.entry((a, b)).or_insert(0) += 1;
                *self.positions.entry(position).or_insert(0) += 1;
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty() && self.length_mismatches == 0
    }

    pub fn render(&self) -> String {
        let mut out = String::from("\nConfusions caractère (agrégées sur le corpus)\n");

        let mut pairs: Vec<(&(char, char), &usize)> = self.pairs.iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(a.1));

        out.push_str("  attendu -> lu    n\n");
        for ((expected, found), count) in pairs {
            out.push_str(&format!(
                "  {} -> {}         {:>4}\n",
                expected, found, count
            ));
        }

        let mut positions: Vec<(&usize, &usize)> = self.positions.iter().collect();
        positions.sort_by(|a, b| b.1.cmp(a.1));

        out.push_str("\nPositions d'erreur (agrégées, sans lien avec les caractères)\n");
        out.push_str("  position    n\n");
        for (position, count) in positions {
            out.push_str(&format!("  {:>8}  {:>4}\n", position, count));
        }

        if self.length_mismatches > 0 {
            out.push_str(&format!(
                "\n  {} lectures de longueur différente, non alignables\n",
                self.length_mismatches
            ));
        }

        out
    }
}

/// Distingue la valeur fausse de la valeur absente.
///
/// Le taux de réussite seul masque un progrès décisif : écarter un candidat erroné sans
/// pour autant trouver le bon ne change rien au taux, alors qu'une donnée absente vaut
/// bien mieux qu'une donnée fausse — celle-ci se propage en aval sans se signaler.
#[derive(Default)]
struct Tally {
    ok: usize,
    wrong: usize,
    total: usize,
}

impl Tally {
    fn add(&mut self, verdict: Verdict) {
        if !verdict.counts() {
            return;
        }

        self.total += 1;

        match verdict {
            Verdict::Ok => self.ok += 1,
            Verdict::Ko => self.wrong += 1,
            _ => {}
        }
    }

    fn render(&self, label: &str) -> String {
        if self.total == 0 {
            return format!("  {:<16} aucun cas mesurable\n", label);
        }

        let wrong = if self.wrong > 0 {
            format!("   dont {} faux", self.wrong)
        } else {
            String::new()
        };

        format!(
            "  {:<16} {:>3}/{:<3} ({:.1} %){}\n",
            label,
            self.ok,
            self.total,
            100.0 * self.ok as f32 / self.total as f32,
            wrong
        )
    }
}

pub struct Report {
    pub files: Vec<FileReport>,
    pub confusion: Confusion,
}

impl Report {
    /// Une ligne par document. Rien de ce qui figure ici ne dépend du contenu reconnu.
    pub fn render_files(&self) -> String {
        let mut out = format!(
            "{:<48} {:<5} {:<5} {:<7} {:<7} {:<10} {:<10} {:<12} {:<6} {:<7} {:<3} {:>7}\n",
            "file",
            "iban",
            "bic",
            "holder",
            "holder~",
            "kind",
            "route",
            "engine",
            "anchor",
            "anchor_h",
            "2p",
            "ms"
        );

        for file in &self.files {
            out.push_str(&format!(
                "{:<48} {:<5} {:<5} {:<7} {:<7} {:<10} {:<10} {:<12} {:<6} {:<7} {:<3} {:>7}\n",
                file.file,
                file.iban.as_str(),
                file.bic.as_str(),
                file.holder_strict.as_str(),
                file.holder_loose.as_str(),
                // nature de l'écart quand le titulaire est faux : dit quels documents
                // méritent une lecture, sans rien dire de leur contenu
                optional(file.holder_mismatch.map(|k| k.as_str())),
                optional(file.route),
                optional(file.engine.or(file.failure.map(|f| f.as_str()))),
                optional(file.anchor),
                optional(file.anchor_height),
                if file.second_pass { "2" } else { "-" },
                file.duration_ms
            ));
            // ventilation par document, quand elle est renseignée
            if file.ocr_calls + file.tess_calls > 0 {
                out.push_str(&format!(
                    "{:<48}   ocr {:>4}× {:>6} ms · tess {:>2}× {:>6} ms · pré {:>5} ms · CP {}/{}/{}\n",
                    "",
                    file.ocr_calls,
                    file.ocr_ms,
                    file.tess_calls,
                    file.tess_ms,
                    file.preprocess_ms,
                    file.postal_anchors,
                    file.holder_candidates,
                    file.holder_blocks_read
                ));
            }
        }

        out
    }

    /// Profil de forme du texte reconnu, ventilé selon que l'IBAN a été trouvé ou non.
    /// C'est le contraste entre les deux groupes qui dit à quoi ressemble un échec, et
    /// la comparaison de ces profils entre corpus qui dit si les échecs se ressemblent.
    pub fn render_profiles(&self) -> String {
        let with_stats = self
            .files
            .iter()
            .filter(|f| !f.known_failure)
            .filter_map(|f| f.text_stats.as_ref().map(|s| (f, s)));

        let groups = grouped(with_stats.map(|(f, s)| {
            let group = match f.iban {
                Verdict::Ok => "IBAN trouvé",
                Verdict::Ko => "IBAN faux",
                Verdict::NotFound => "IBAN non trouvé",
                Verdict::NoTruth => "sans vérité",
            };
            (group, s)
        }));

        if groups.is_empty() {
            return String::new();
        }

        let mut out = String::new();
        for (group, stats) in groups {
            out.push_str(&ProfileSummary::of(&stats).render(group));
        }

        out
    }

    pub fn render_summary(&self) -> String {
        let measurable: Vec<&FileReport> = self.files.iter().filter(|f| !f.known_failure).collect();

        let mut iban = Tally::default();
        let mut bic = Tally::default();
        let mut strict = Tally::default();
        let mut loose = Tally::default();
        let mut content = Tally::default();

        for file in &measurable {
            iban.add(file.iban);
            bic.add(file.bic);
            strict.add(file.holder_strict);
            loose.add(file.holder_loose);
            content.add(file.holder_content);
        }

        let mut out = format!("\n{} documents analysés\n\n", self.files.len());

        out.push_str("Taux de réussite\n");
        out.push_str(&iban.render("IBAN"));
        out.push_str(&bic.render("BIC"));
        out.push_str(&strict.render("titulaire"));
        out.push_str(&loose.render("titulaire souple"));
        out.push_str(&content.render("titulaire contenu"));

        let panicked = self
            .files
            .iter()
            .filter(|f| f.failure == Some(Failure::Panicked))
            .count();

        if panicked > 0 {
            out.push_str(&format!(
                "\n  {} document(s) ont fait paniquer le pipeline — voir la colonne engine\n",
                panicked
            ));
        }

        let known: Vec<&FileReport> = self.files.iter().filter(|f| f.known_failure).collect();
        if !known.is_empty() {
            let unexpected = known.iter().filter(|f| f.iban.is_ok()).count();
            out.push_str(&format!(
                "\n  {} cas d'échec connu, hors du calcul ({} réussissent malgré tout)\n",
                known.len(),
                unexpected
            ));
        }

        out.push_str(
            &self.render_grouped("Réussite IBAN par forme d'entrée", |f| {
                f.src.clone().unwrap_or_else(|| "-".to_string())
            }),
        );

        out.push_str(&self.render_grouped("Réussite IBAN par recette", |f| {
            f.recipe.clone().unwrap_or_else(|| "-".to_string())
        }));

        out.push_str(&self.render_holder_mismatches());
        out.push_str(&self.render_engines());
        out.push_str(&self.render_durations());

        out
    }

    /// Taux de réussite ventilé : c'est ce qui transforme un chiffre global en
    /// diagnostic exploitable.
    fn render_grouped(&self, title: &str, key: fn(&FileReport) -> String) -> String {
        let mut groups: BTreeMap<String, Tally> = BTreeMap::new();

        for file in self.files.iter().filter(|f| !f.known_failure) {
            groups.entry(key(file)).or_default().add(file.iban);
        }

        if groups.len() <= 1 {
            return String::new();
        }

        let mut out = format!("\n{}\n", title);
        for (group, tally) in groups {
            out.push_str(&tally.render(&group));
        }

        out
    }

    /// Comment le titulaire est faux, quand il l'est. Chaque catégorie appelle un
    /// correctif différent — d'où l'intérêt de les compter séparément.
    fn render_holder_mismatches(&self) -> String {
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();

        for file in &self.files {
            if let Some(kind) = file.holder_mismatch {
                *kinds.entry(kind.as_str()).or_insert(0) += 1;
            }
        }

        if kinds.is_empty() {
            return String::new();
        }

        let mut out = String::from("\nNature des titulaires faux\n");
        for (kind, count) in kinds {
            out.push_str(&format!("  {:<16} {:>3}\n", kind, count));
        }

        out
    }

    /// Quelles branches du pipeline produisent réellement les succès : les absentes
    /// sont des candidates à la suppression.
    fn render_engines(&self) -> String {
        let mut engines: BTreeMap<&str, usize> = BTreeMap::new();

        for file in &self.files {
            *engines.entry(file.engine.unwrap_or("aucune")).or_insert(0) += 1;
        }

        let mut out = String::from("\nStratégie ayant abouti\n");
        for (engine, count) in engines {
            out.push_str(&format!("  {:<16} {:>3}\n", engine, count));
        }

        let second_pass = self.files.iter().filter(|f| f.second_pass).count();
        out.push_str(&format!("  {:<16} {:>3}\n", "(2e passe)", second_pass));

        out
    }

    fn render_durations(&self) -> String {
        let mut durations: Vec<u64> = self.files.iter().map(|f| f.duration_ms).collect();

        if durations.is_empty() {
            return String::new();
        }

        durations.sort_unstable();

        let median = durations[durations.len() / 2];
        let p95 = durations[(durations.len() * 95 / 100).min(durations.len() - 1)];
        let total: u64 = durations.iter().sum();

        let sum = |f: fn(&FileReport) -> u64| self.files.iter().map(f).sum::<u64>();
        let (ocr, tess, pre, pop) = (
            sum(|f| f.ocr_ms),
            sum(|f| f.tess_ms),
            sum(|f| f.preprocess_ms),
            sum(|f| f.poppler_ms),
        );
        let calls_ocr: u32 = self.files.iter().map(|f| f.ocr_calls).sum();
        let calls_tess: u32 = self.files.iter().map(|f| f.tess_calls).sum();
        let pct = |ms: u64| {
            if total == 0 {
                0.0
            } else {
                100.0 * ms as f32 / total as f32
            }
        };

        format!(
            "\nDurées\n  médiane {} ms · p95 {} ms · total {:.1} s\n\
             \n  Ventilation du temps\n\
             \x20 ppocr         {:>7.1} s  ({:>4.1} %)  {:>4} appels\n\
             \x20 tesseract     {:>7.1} s  ({:>4.1} %)  {:>4} appels\n\
             \x20 prétraitement {:>7.1} s  ({:>4.1} %)\n\
             \x20 poppler       {:>7.1} s  ({:>4.1} %)\n",
            median,
            p95,
            total as f32 / 1000.0,
            ocr as f32 / 1000.0,
            pct(ocr),
            calls_ocr,
            tess as f32 / 1000.0,
            pct(tess),
            calls_tess,
            pre as f32 / 1000.0,
            pct(pre),
            pop as f32 / 1000.0,
            pct(pop),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(file: &str) -> FileReport {
        FileReport {
            file: file.to_string(),
            iban: Verdict::Ok,
            bic: Verdict::Ok,
            holder_strict: Verdict::Ko,
            holder_loose: Verdict::Ok,
            holder_content: Verdict::Ok,
            known_failure: false,
            src: Some("pdf_img".to_string()),
            recipe: Some("h20".to_string()),
            route: Some("pdf_image"),
            engine: Some("ppocr:page"),
            anchor: Some("ppocr"),
            anchor_height: Some(21),
            angle_deg: Some(-0.4),
            second_pass: false,
            postal_anchors: 0,
            holder_candidates: 0,
            holder_blocks_read: 0,
            width: 1240,
            height: 1754,
            duration_ms: 870,
            ocr_ms: 0,
            ocr_calls: 0,
            tess_ms: 0,
            tess_calls: 0,
            preprocess_ms: 0,
            poppler_ms: 0,
            failure: None,
            text_stats: None,
            holder_mismatch: None,
        }
    }

    #[test]
    fn rates_ignore_fields_without_truth() {
        let mut without_truth = report("b.pdf");
        without_truth.iban = Verdict::NoTruth;

        let rendered = Report {
            files: vec![report("a.pdf"), without_truth],
            confusion: Confusion::default(),
        }
        .render_summary();

        // un seul IBAN mesurable sur les deux documents
        assert!(rendered.contains("IBAN               1/1"), "{}", rendered);
    }

    /// Écarter un candidat erroné sans trouver le bon laisse le taux inchangé : seul le
    /// décompte des valeurs fausses rend le progrès visible.
    #[test]
    fn wrong_values_are_counted_apart_from_the_rate() {
        let mut wrong = report("b.pdf");
        wrong.bic = Verdict::Ko;

        let mut missing = report("c.pdf");
        missing.bic = Verdict::NotFound;

        let with_wrong = Report {
            files: vec![wrong],
            confusion: Confusion::default(),
        }
        .render_summary();

        let with_missing = Report {
            files: vec![missing],
            confusion: Confusion::default(),
        }
        .render_summary();

        // même taux des deux côtés, mais un seul signale une valeur fausse
        let bic_line = |rendered: &str| {
            rendered
                .lines()
                .find(|line| line.trim_start().starts_with("BIC"))
                .unwrap()
                .to_string()
        };

        let (wrong_line, missing_line) = (bic_line(&with_wrong), bic_line(&with_missing));

        assert!(wrong_line.contains("0/1"), "{}", wrong_line);
        assert!(missing_line.contains("0/1"), "{}", missing_line);
        assert!(wrong_line.contains("dont 1 faux"), "{}", wrong_line);
        assert!(!missing_line.contains("faux"), "{}", missing_line);
    }

    #[test]
    fn known_failures_stay_visible_but_out_of_the_rate() {
        let mut known = report("c.pdf");
        known.known_failure = true;
        known.iban = Verdict::NotFound;

        let rendered = Report {
            files: vec![report("a.pdf"), known],
            confusion: Confusion::default(),
        }
        .render_summary();

        assert!(rendered.contains("IBAN               1/1"), "{}", rendered);
        assert!(rendered.contains("1 cas d'échec connu"), "{}", rendered);
    }

    #[test]
    fn confusion_pairs_and_positions_are_never_joined() {
        let mut confusion = Confusion::default();
        confusion.observe("FR7630001000644919009562088", "FR763O001000644919009562088");
        confusion.observe("FR7630001000644919009562088", "FR763O001000644919009562088");

        let rendered = confusion.render();

        assert!(rendered.contains("0 -> O"));
        assert!(rendered.contains("Positions d'erreur"));

        // la position n'apparaît jamais sur la même ligne qu'un caractère
        for line in rendered.lines().filter(|l| l.contains("->")) {
            assert!(
                !line.contains('5'),
                "position et caractère joints : {}",
                line
            );
        }
    }

    #[test]
    fn confusion_skips_unalignable_lengths() {
        let mut confusion = Confusion::default();
        confusion.observe("FR7630001", "FR76300");

        assert!(confusion.pairs.is_empty());
        assert!(confusion.render().contains("longueur différente"));
    }

    /// Le rendu par document ne doit contenir que des étiquettes et des nombres.
    #[test]
    fn file_lines_carry_no_recognised_text() {
        let rendered = Report {
            files: vec![report("a.pdf")],
            confusion: Confusion::default(),
        }
        .render_files();

        assert!(rendered.contains("a.pdf"));
        assert!(rendered.contains("ppocr:page"));
        assert!(!rendered.contains("FR76"));
    }
}
