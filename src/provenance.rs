//! Trace du chemin suivi pour extraire un RIB.
//!
//! Le pipeline enchaîne plusieurs stratégies jusqu'à ce que l'une aboutisse, sans
//! jamais dire laquelle. Sans cette trace, impossible de savoir quelles branches
//! servent réellement, donc lesquelles méritent d'être améliorées ou retirées.
//!
//! Ne contient que des étiquettes et des grandeurs géométriques : aucun texte reconnu,
//! de sorte qu'une trace puisse être publiée sans divulguer le contenu du document.

/// Branche d'aiguillage retenue par `analysis::vec_to_rib`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Texte extrait du PDF, sans OCR.
    PdfText,
    /// PDF rasterisé faute de texte exploitable.
    PdfImage,
    /// Image fournie telle quelle.
    Image,
    /// Fichier texte brut.
    PlainText,
}

/// Moteur ayant fourni l'ancre de localisation de l'IBAN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorSource {
    PpOcr,
    Tesseract,
}

/// Stratégie ayant effectivement produit l'IBAN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// `pdftotext`, sans OCR.
    PdfText,
    /// OCR de la page entière par PP-OCR.
    PpOcrPage,
    /// Recadrage autour d'une ancre PP-OCR.
    PpOcrCrop,
    /// Recadrage étroit, pour les IBAN espacés de façon inhabituelle.
    PpOcrNarrowCrop,
    /// Recadrage autour d'une ancre tesseract, après redressement éventuel.
    TessCrop,
}

impl Route {
    pub fn as_str(&self) -> &'static str {
        match self {
            Route::PdfText => "pdf_text",
            Route::PdfImage => "pdf_image",
            Route::Image => "image",
            Route::PlainText => "plain_text",
        }
    }
}

impl AnchorSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorSource::PpOcr => "ppocr",
            AnchorSource::Tesseract => "tess",
        }
    }
}

impl Engine {
    pub fn as_str(&self) -> &'static str {
        match self {
            Engine::PdfText => "pdf_text",
            Engine::PpOcrPage => "ppocr:page",
            Engine::PpOcrCrop => "ppocr:crop",
            Engine::PpOcrNarrowCrop => "ppocr:narrow",
            Engine::TessCrop => "tess:crop",
        }
    }
}

/// Ce qu'on retient du texte reconnu : des comptes, jamais des caractères.
///
/// Sert à comparer la nature des défaillances entre un corpus synthétique et un corpus
/// réel qu'on ne peut pas lire — quand seuls les taux se comparent, on corrige des
/// défauts qui n'existent que dans le corpus généré.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TextStats {
    pub lines: u32,
    pub words: u32,
    pub chars: u32,
    pub digits: u32,
    pub alphas: u32,
    pub symbols: u32,
    pub short_lines: u32,
    pub single_char_words: u32,
    pub mixed_words: u32,
    pub vocabulary_hits: u32,
    pub has_iban_prefix: bool,
    pub has_postal_code: bool,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Provenance {
    pub route: Option<Route>,
    /// Statistiques de forme du texte reconnu sur la page, première passe. Un observateur
    /// facultatif les calcule à la volée : le texte lui-même n'est jamais conservé.
    pub page_text_stats: Option<TextStats>,
    pub anchor: Option<AnchorSource>,
    /// Hauteur de l'ancre en pixels : c'est la grandeur qui décide si le texte est
    /// assez grand pour être reconnu.
    pub anchor_height: Option<u32>,
    /// Inclinaison détectée par tesseract, en degrés.
    pub angle_deg: Option<f32>,
    pub engine: Option<Engine>,
    /// Vrai si le résultat vient de la seconde passe, sur image nettoyée.
    pub second_pass: bool,
    pub image_width: u32,
    pub image_height: u32,
    /// Ventilation du temps par étape, relevée en fin d'analyse.
    pub timings: crate::timing::Timings,
    /// Codes postaux détectés sur la page, candidats retenus après tri, blocs lus.
    /// Trois comptes qui disent où la recherche du titulaire s'arrête.
    pub postal_anchors: u32,
    pub holder_candidates: u32,
    pub holder_blocks_read: u32,
}

impl TextStats {
    /// Comptages de forme. Les termes cherchés sont ceux dont dépend l'ancrage du
    /// titulaire et la distinction titulaire/domiciliation.
    pub fn of(text: &str) -> Self {
        const VOCABULARY: [&str; 12] = [
            "IBAN",
            "BIC",
            "TITULAIRE",
            "INTITULE",
            "COMPTE",
            "BANQUE",
            "GUICHET",
            "DOMICILIATION",
            "RELEVE",
            "IDENTITE",
            "AGENCE",
            "CLE",
        ];

        fn fold(c: char) -> char {
            match c {
                'é' | 'è' | 'ê' | 'ë' => 'E',
                'à' | 'â' | 'ä' => 'A',
                'î' | 'ï' => 'I',
                'ô' | 'ö' => 'O',
                'ù' | 'û' | 'ü' => 'U',
                'ç' => 'C',
                c => c.to_ascii_uppercase(),
            }
        }

        let upper: String = text.chars().map(fold).collect();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let words: Vec<&str> = text.split_whitespace().collect();
        let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();

        let count = |f: fn(&char) -> bool| chars.iter().filter(|c| f(c)).count() as u32;

        let is_iban_like = |w: &str| {
            let w = w.to_ascii_uppercase();
            let b = w.as_bytes();
            (b.len() >= 4 && &b[..2] == b"FR" && b[2].is_ascii_digit() && b[3].is_ascii_digit())
                || (b.len() >= 6
                    && b[..4].iter().all(|c| c.is_ascii_uppercase())
                    && &b[4..6] == b"FR")
        };

        let mixed_words = words
            .iter()
            .filter(|w| w.chars().count() >= 3 && !is_iban_like(w))
            .filter(|w| {
                w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_alphabetic())
            })
            .count() as u32;

        let has_iban_prefix = upper
            .as_bytes()
            .windows(4)
            .any(|w| &w[..2] == b"FR" && w[2].is_ascii_digit() && w[3].is_ascii_digit());

        // cinq chiffres, un blanc, puis au moins deux lettres
        let has_postal_code = upper
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| {
                pair[0].len() == 5
                    && pair[0].bytes().all(|b| b.is_ascii_digit())
                    && pair[1].chars().take(2).all(|c| c.is_ascii_uppercase())
                    && pair[1].len() >= 2
            });

        TextStats {
            lines: lines.len() as u32,
            words: words.len() as u32,
            chars: chars.len() as u32,
            digits: count(|c| c.is_ascii_digit()),
            alphas: count(|c| c.is_alphabetic()),
            symbols: count(|c| !c.is_alphanumeric()),
            short_lines: lines
                .iter()
                .filter(|l| l.trim().chars().count() < 3)
                .count() as u32,
            single_char_words: words.iter().filter(|w| w.chars().count() == 1).count() as u32,
            mixed_words,
            vocabulary_hits: VOCABULARY.iter().filter(|t| upper.contains(*t)).count() as u32,
            has_iban_prefix,
            has_postal_code,
        }
    }
}

impl TextStats {
    /// Vrai quand la première lecture n'a presque rien rendu : trop peu de caractères
    /// et aucun terme de RIB. Le document n'est pas un RIB mal lu, c'est une image sur
    /// laquelle il n'y a rien à lire — vignette perdue dans une page blanche, photo
    /// illisible. Le dire vaut mieux que rendre un vide indiscernable d'un RIB absent.
    pub fn is_unreadable(&self) -> bool {
        self.digits + self.alphas < 20 && self.vocabulary_hits == 0 && !self.has_iban_prefix
    }
}

impl Provenance {
    pub fn route(route: Route) -> Self {
        Provenance {
            route: Some(route),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_read_is_unreadable_a_real_rib_is_not() {
        assert!(TextStats::of("R.\n|\n~").is_unreadable());
        assert!(TextStats::of("").is_unreadable());
        assert!(
            !TextStats::of("Titulaire du compte\nM MATISSE HENRI\n44100 NANTES").is_unreadable()
        );
        // peu de texte mais un préfixe d'IBAN : on a peut-être juste mal lu
        assert!(!TextStats::of("FR76 3000").is_unreadable());
    }
}
