//! Profil de forme du texte reconnu, agrégé sur un corpus.
//!
//! Le taux de réussite transfère du corpus synthétique au réel, mais la nature des
//! échecs, elle, ne transfère pas : deux chantiers ont corrigé des défaillances qui
//! n'existaient que dans le corpus généré. Comparer les taux ne suffit donc pas — il faut
//! comparer ce que l'OCR produit.
//!
//! Le pipeline consigne pour chaque document des comptages de forme (`TextStats`) —
//! proportion de chiffres, longueur des lignes, part de vocabulaire retrouvé,
//! fragmentation — sans conserver le texte. Ce module les résume en distributions, que
//! deux corpus se comparent directement.

use std::collections::BTreeMap;

use crate::provenance::TextStats;

#[derive(Debug, Default, Clone)]
pub struct Distribution {
    pub mean: f32,
    pub p25: f32,
    pub median: f32,
    pub p75: f32,
}

impl Distribution {
    fn of(mut values: Vec<f32>) -> Self {
        if values.is_empty() {
            return Distribution::default();
        }

        values.sort_by(|a, b| a.partial_cmp(b).unwrap());

        let at = |q: f32| values[((values.len() - 1) as f32 * q).round() as usize];

        Distribution {
            mean: values.iter().sum::<f32>() / values.len() as f32,
            p25: at(0.25),
            median: at(0.5),
            p75: at(0.75),
        }
    }
}

/// Distributions des grandeurs de forme sur un ensemble de documents.
#[derive(Debug, Default)]
pub struct ProfileSummary {
    pub count: usize,
    pub metrics: Vec<(&'static str, Distribution)>,
}

fn ratio(n: u32, d: u32) -> f32 {
    if d == 0 {
        0.0
    } else {
        n as f32 / d as f32
    }
}

impl ProfileSummary {
    pub fn of(stats: &[&TextStats]) -> Self {
        let collect =
            |f: fn(&TextStats) -> f32| Distribution::of(stats.iter().map(|s| f(s)).collect());

        let metrics = vec![
            ("lignes", collect(|s| s.lines as f32)),
            ("mots", collect(|s| s.words as f32)),
            ("caractères", collect(|s| s.chars as f32)),
            ("part chiffres", collect(|s| ratio(s.digits, s.chars))),
            ("part lettres", collect(|s| ratio(s.alphas, s.chars))),
            ("part symboles", collect(|s| ratio(s.symbols, s.chars))),
            ("long. ligne", collect(|s| ratio(s.chars, s.lines))),
            ("lignes courtes", collect(|s| ratio(s.short_lines, s.lines))),
            (
                "mots 1 car.",
                collect(|s| ratio(s.single_char_words, s.words)),
            ),
            ("mots mixtes", collect(|s| ratio(s.mixed_words, s.words))),
            ("vocabulaire /12", collect(|s| s.vocabulary_hits as f32)),
            (
                "préfixe IBAN vu",
                collect(|s| if s.has_iban_prefix { 1.0 } else { 0.0 }),
            ),
            (
                "code postal vu",
                collect(|s| if s.has_postal_code { 1.0 } else { 0.0 }),
            ),
        ];

        ProfileSummary {
            count: stats.len(),
            metrics,
        }
    }

    pub fn render(&self, title: &str) -> String {
        let mut out = format!(
            "\n{} ({} documents)\n  {:<16} {:>8} {:>8} {:>8} {:>8}\n",
            title, self.count, "", "moyenne", "p25", "médiane", "p75"
        );

        for (name, d) in &self.metrics {
            out.push_str(&format!(
                "  {:<16} {:>8.2} {:>8.2} {:>8.2} {:>8.2}\n",
                name, d.mean, d.p25, d.median, d.p75
            ));
        }

        out
    }
}

/// Résumés par groupe : les documents où l'IBAN a été trouvé contre ceux où il ne l'a
/// pas été. C'est le contraste entre les deux qui dit à quoi ressemble un échec.
pub fn grouped<'a>(
    stats: impl Iterator<Item = (&'a str, &'a TextStats)>,
) -> BTreeMap<&'a str, Vec<&'a TextStats>> {
    let mut groups: BTreeMap<&str, Vec<&TextStats>> = BTreeMap::new();

    for (group, stat) in stats {
        groups.entry(group).or_default().push(stat);
    }

    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_rib_has_a_recognisable_shape() {
        let text = "RELEVE D'IDENTITE BANCAIRE\nTitulaire du compte\nM MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES\nIBAN FR76 3000 1000 6449 1900 9562 088\nBIC BDFEFRPPCCT";
        let stats = TextStats::of(text);

        assert_eq!(stats.lines, 7);
        assert!(stats.vocabulary_hits >= 5, "{:?}", stats);
        assert!(stats.has_iban_prefix);
        assert!(stats.has_postal_code);
        assert!(ratio(stats.symbols, stats.chars) < 0.05);
        assert_eq!(stats.short_lines, 0);
    }

    #[test]
    fn ocr_noise_shows_as_fragmentation_and_symbols() {
        let text = "R.\n|\nT1tula1re\nM\n~\nFR76 3O00\n:\n.\n4A100 N4NTES";
        let stats = TextStats::of(text);

        assert!(ratio(stats.short_lines, stats.lines) > 0.4, "{:?}", stats);
        assert!(ratio(stats.symbols, stats.chars) > 0.05, "{:?}", stats);
        assert!(ratio(stats.mixed_words, stats.words) > 0.2, "{:?}", stats);
    }

    /// Les statistiques ne doivent jamais contenir de texte : que des nombres.
    #[test]
    fn stats_carry_no_content() {
        let stats = TextStats::of("M MATISSE HENRI FR7630001000644919009562088");
        let debug = format!("{:?}", stats);

        assert!(!debug.contains("MATISSE"));
        assert!(!debug.contains("FR76"));
    }

    #[test]
    fn distribution_quartiles_are_ordered() {
        let d = Distribution::of(vec![5.0, 1.0, 3.0, 4.0, 2.0]);

        assert_eq!(d.median, 3.0);
        assert!(d.p25 <= d.median && d.median <= d.p75);
        assert!((d.mean - 3.0).abs() < 1e-6);
    }

    #[test]
    fn summary_renders_one_line_per_metric() {
        let a = TextStats::of("IBAN FR76 3000\n44100 NANTES");
        let b = TextStats::of("R.\n|");
        let summary = ProfileSummary::of(&[&a, &b]);

        assert_eq!(summary.count, 2);
        let rendered = summary.render("test");
        assert!(rendered.contains("vocabulaire /12"));
        assert!(rendered.contains("lignes courtes"));
    }
}
