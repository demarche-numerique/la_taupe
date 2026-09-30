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

/// Sort d'un bloc candidat au titulaire, sur le chemin image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    /// Écarté sur la lecture pleine page, sans recadrage : son voisinage se présente
    /// comme une domiciliation.
    SkippedAsDomiciliation,
    /// Recadré, puis écarté : c'est une domiciliation.
    Domiciliation,
    /// Recadré, mais ni civilité, ni forme juridique, ni libellé ne le désignent.
    NotDesignated,
    /// Désigné, mais rien n'a tenu une fois borné au code postal : bloc douteux.
    Trimmed,
    /// Recevable, mais un autre bloc l'a emporté.
    Outranked,
    /// Rendu comme titulaire.
    Kept,
}

impl BlockOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockOutcome::SkippedAsDomiciliation => "écarté avant lecture (domiciliation)",
            BlockOutcome::Domiciliation => "relu, écarté (domiciliation)",
            BlockOutcome::NotDesignated => "relu, écarté (ni civilité ni libellé)",
            BlockOutcome::Trimmed => "relu, écarté (douteux une fois borné)",
            BlockOutcome::Outranked => "relu, supplanté par un autre bloc",
            BlockOutcome::Kept => "retenu",
        }
    }

    /// Code stable, pour la trace JSON.
    pub fn code(&self) -> &'static str {
        match self {
            BlockOutcome::SkippedAsDomiciliation => "skipped_as_domiciliation",
            BlockOutcome::Domiciliation => "domiciliation",
            BlockOutcome::NotDesignated => "not_designated",
            BlockOutcome::Trimmed => "trimmed",
            BlockOutcome::Outranked => "outranked",
            BlockOutcome::Kept => "kept",
        }
    }
}

/// Bloc candidat au titulaire : sa place et ce qu'il est devenu. La place est en
/// fractions de la page d'origine — x0, y0, x1, y1 —, pour se comparer à un rectangle
/// annoté ; elle manque quand l'image lue ne se ramène pas à la page par un quart de tour
/// (redressement d'un angle quelconque par tesseract). Aucun texte.
#[derive(Debug, Clone, PartialEq)]
pub struct HolderBlock {
    pub rect: Option<[f32; 4]>,
    pub outcome: BlockOutcome,
}

/// Ligne lue par l'OCR sur la page où le titulaire est cherché : sa place (fractions de
/// la page d'origine, comme les blocs), si elle a la forme d'une ligne de code postal, et
/// si elle est écartée des ancres pour sa hauteur — le détecteur a fusionné plusieurs
/// lignes. Aucun texte.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub rect: Option<[f32; 4]>,
    pub postal: bool,
    pub oversized: bool,
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
    /// Les blocs candidats au titulaire, avec leur place et leur sort (chemin image).
    pub holder_blocks: Vec<HolderBlock>,
    /// Les lignes lues sur la page où le titulaire est cherché (chemin image).
    pub ocr_lines: Vec<OcrLine>,
    /// Page du PDF rastérisée pour l'OCR (1 pour une image).
    pub page: Option<u32>,
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

    /// La trace en JSON, pour `la_taupe --trace` : des étiquettes, des comptes et des
    /// rectangles, jamais de texte — elle peut accompagner un résultat sans rien
    /// divulguer de plus que lui.
    pub fn to_json(&self) -> serde_json::Value {
        let blocks: Vec<serde_json::Value> = self
            .holder_blocks
            .iter()
            .map(|b| {
                // quatre décimales : un dixième de millimètre sur une page A4
                let rect = b
                    .rect
                    .map(|r| r.map(|v| (f64::from(v) * 1e4).round() / 1e4));
                serde_json::json!({ "rect": rect, "outcome": b.outcome.code() })
            })
            .collect();

        let lines: Vec<serde_json::Value> = self
            .ocr_lines
            .iter()
            .map(|l| {
                let rect = l
                    .rect
                    .map(|r| r.map(|v| (f64::from(v) * 1e4).round() / 1e4));
                serde_json::json!({ "rect": rect, "postal": l.postal, "oversized": l.oversized })
            })
            .collect();

        serde_json::json!({
            "route": self.route.map(|r| r.as_str()),
            "engine": self.engine.map(|e| e.as_str()),
            "page": self.page,
            "second_pass": self.second_pass,
            "postal_anchors": self.postal_anchors,
            "holder_candidates": self.holder_candidates,
            "holder_blocks_read": self.holder_blocks_read,
            "holder_blocks": blocks,
            "ocr_lines": lines,
        })
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

    /// La trace JSON ne porte que des étiquettes, des comptes et des rectangles.
    #[test]
    fn the_json_trace_carries_places_and_outcomes() {
        let provenance = Provenance {
            route: Some(Route::Image),
            page: Some(1),
            holder_blocks_read: 1,
            holder_blocks: vec![
                HolderBlock {
                    rect: Some([0.1, 0.2, 0.300_04, 0.4]),
                    outcome: BlockOutcome::Kept,
                },
                HolderBlock {
                    rect: None,
                    outcome: BlockOutcome::NotDesignated,
                },
            ],
            ..Default::default()
        };

        let json = provenance.to_json();
        assert_eq!(json["route"], "image");
        assert_eq!(json["page"], 1);
        assert_eq!(json["holder_blocks"][0]["outcome"], "kept");
        assert_eq!(json["holder_blocks"][0]["rect"][2], 0.3);
        assert!(json["holder_blocks"][1]["rect"].is_null());
    }
}
