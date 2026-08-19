//! PP-OCR, le moteur de reconnaissance de page.
//!
//! Mesuré dans la cascade sur les deux corpus réels, contre ocrs qu'il remplace :
//! mieux sur tous les champs, deux fois moins de titulaires faux sur les photos,
//! médiane divisée par 3,4. L'adaptateur rend des `TextLine` positionnées mot à mot ;
//! tesseract reste le repli sur les recadrages d'IBAN.

use std::sync::OnceLock;

use image::DynamicImage;
use oar_ocr::oarocr::{OAROCRBuilder, OAROCR};
use regex::Regex;

use crate::lines::{extract_anchors, lines_to_text, Rect, TextChar, TextLine};
use crate::shapes::Anchor;

/// Détection et reconnaissance, avec positions par mot. Sans classification
/// d'orientation : mesurée sur les corpus réels, elle coûte sans rien rapporter — les
/// documents pivotés sont déjà lus sans elle.
static ENGINE: OnceLock<OAROCR> = OnceLock::new();

/// Modèles PP-OCR v6 tiny, embarqués dans le binaire au build — téléchargés une fois
/// par `download-models.sh`. Le binaire est autonome : rien n'est
/// téléchargé à l'exécution, ce qu'une prod hors ligne exige.
///
/// v6 tiny est le choix sur mesure : sur les deux corpus réels il fait mieux qu'ocrs sur
/// tous les champs, deux fois moins de titulaires faux sur les photos, médiane 3,4 fois
/// plus courte. v6 small lit deux BIC de plus sur les photos mais perd sur le titulaire
/// et coûte le double ; v5 server met quarante secondes la page en CPU.
const DET_MODEL: &[u8] = include_bytes!("../models/pp-ocrv6_tiny_det.onnx");
const REC_MODEL: &[u8] = include_bytes!("../models/pp-ocrv6_tiny_rec.onnx");
const DICT: &[u8] = include_bytes!("../models/ppocrv6_dict.txt");

/// oar-ocr lit les modèles ONNX en mémoire mais exige un chemin pour le dictionnaire :
/// il est écrit une fois dans le répertoire temporaire.
fn dict_path() -> std::path::PathBuf {
    let path = std::env::temp_dir().join("la_taupe_ppocrv6_dict.txt");
    let up_to_date = std::fs::metadata(&path)
        .map(|m| m.len() as usize == DICT.len())
        .unwrap_or(false);
    if !up_to_date {
        std::fs::write(&path, DICT).expect("écriture du dictionnaire PP-OCR");
    }
    path
}

/// Pour essayer un autre jeu de modèles sans le rebuild : `LA_TAUPE_PPOCR_MODEL_DIR`
/// pointe un répertoire contenant `det.onnx`, `rec.onnx` et `dict.txt`. Aucun
/// téléchargement implicite.
fn engine() -> &'static OAROCR {
    ENGINE.get_or_init(|| {
        #[allow(clippy::const_is_empty)]
        if DET_MODEL.is_empty() || REC_MODEL.is_empty() || DICT.is_empty() {
            panic!("--> PP-OCR models are empty in models/ directory. Please run `download-models.sh` to download the models.");
        }

        let builder = match std::env::var("LA_TAUPE_PPOCR_MODEL_DIR") {
            Ok(dir) => {
                let dir = std::path::PathBuf::from(dir);
                log::info!("PP-OCR : modèles depuis {}", dir.display());
                OAROCRBuilder::new(dir.join("det.onnx"), dir.join("rec.onnx"), dir.join("dict.txt"))
            }
            Err(_) => OAROCRBuilder::new(DET_MODEL, REC_MODEL, dict_path()),
        };

        builder
            .return_word_box(true)
            .build()
            .expect("moteur PP-OCR : modèles invalides ou ONNX Runtime absent")
    })
}

/// Le détecteur de PP-OCR dilate ses boîtes (« unclip ») : mesurées sur les mêmes
/// lignes, elles font deux fois la hauteur de celles d'ocrs — 76 px de médiane contre
/// 37, 66 contre 44 sur une ligne d'IBAN. Or tous les masques de la cascade sont des
/// multiples de la hauteur d'ancre : sans correction, ils débordent d'un facteur deux et
/// le titulaire des photos tombe de 62 à 12 pour cent. Chaque boîte est donc resserrée
/// verticalement autour de son centre, pour rendre la hauteur du texte et non celle du
/// détecteur.
const VERTICAL_UNCLIP: f32 = 2.0;

/// Rectangle englobant d'un polygone, resserré verticalement.
fn bounding(points: &[oar_ocr::processors::Point]) -> Option<Rect> {
    if points.is_empty() {
        return None;
    }
    let min_x = points.iter().map(|p| p.x).fold(f32::MAX, f32::min);
    let min_y = points.iter().map(|p| p.y).fold(f32::MAX, f32::min);
    let max_x = points.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    let max_y = points.iter().map(|p| p.y).fold(f32::MIN, f32::max);

    let center_y = (min_y + max_y) / 2.0;
    let height = ((max_y - min_y) / VERTICAL_UNCLIP).max(1.0);

    Some(Rect::from_tlhw(
        (center_y - height / 2.0).round() as i32,
        min_x.round() as i32,
        height.round() as i32,
        (max_x - min_x).round().max(1.0) as i32,
    ))
}

/// PP-OCR colle un nombre au mot qui le suit — « 222AVENUE », « 44800ST » — trait d'un
/// modèle entraîné sur des écritures sans espace. Sur un RIB français, la frontière
/// chiffre→lettre est toujours un espace : numéro de voie, code postal. On la rétablit
/// au découpage en mots. L'IBAN et le BIC ne passent pas par ici pour leur extraction,
/// qui a ses propres motifs tolérants aux espaces.
fn split_digit_letter(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();

    // Seul un nombre en tête de mot est séparé de ce qui le suit. Un chiffre au milieu
    // d'un identifiant — le « 2A » de CMCIFR2A, le « 1XXX » de FTNOFRP1XXX — n'est pas
    // une frontière de mot.
    let leading_digits = chars.iter().take_while(|c| c.is_ascii_digit()).count();

    if leading_digits >= 2 && leading_digits < chars.len() && chars[leading_digits].is_alphabetic()
    {
        vec![
            chars[..leading_digits].iter().collect(),
            chars[leading_digits..].iter().collect(),
        ]
    } else {
        vec![word.to_string()]
    }
}

/// Répartit les caractères d'un mot uniformément dans sa boîte : ocrs donne une boîte
/// par caractère, PP-OCR une par mot. Les positions intra-mot sont donc approchées,
/// ce qui suffit à tout ce que la cascade en fait — ancrage, recadrage, projection.
fn spread(word: &str, rect: Rect) -> Vec<TextChar> {
    let n = word.chars().count().max(1) as i32;
    let width = (rect.width() / n).max(1);

    word.chars()
        .enumerate()
        .map(|(i, c)| TextChar {
            char: c,
            rect: Rect::from_tlhw(
                rect.top(),
                rect.left() + i as i32 * width,
                rect.height(),
                width,
            ),
        })
        .collect()
}

/// Reconnaît la page et rend des lignes positionnées.
pub fn recognize(img: &DynamicImage) -> Vec<TextLine> {
    crate::timing::measure(crate::timing::ocr, || recognize_inner(img))
}

/// Reconnaît la page et rend le texte joint, ligne par ligne.
pub fn image_to_string(img: DynamicImage) -> String {
    lines_to_text(&recognize(&img))
}

/// Reconnaît la page et rend le texte, les lignes positionnées et les ancres du motif.
pub fn recognize_anchors(
    img: &DynamicImage,
    word_regex: &Regex,
    line_regex: Option<&Regex>,
) -> (String, Vec<TextLine>, Vec<Anchor>) {
    let text_lines = recognize(img);
    let text = lines_to_text(&text_lines);
    let anchors = extract_anchors(text_lines.clone(), word_regex, line_regex);

    (text, text_lines, anchors)
}

fn recognize_inner(img: &DynamicImage) -> Vec<TextLine> {
    recognize_with_engine(engine(), img)
}

fn recognize_with_engine(engine: &OAROCR, img: &DynamicImage) -> Vec<TextLine> {
    let rgb = img.to_rgb8();
    let results = match engine.predict(vec![rgb]) {
        Ok(results) => results,
        Err(e) => {
            log::warn!("PP-OCR : {}", e);
            return Vec::new();
        }
    };

    let mut lines: Vec<TextLine> = Vec::new();

    for page in &results {
        for region in &page.text_regions {
            let Some(text) = region.text.as_deref() else {
                continue;
            };
            // Le dictionnaire de PP-OCRv6 rend parfois l'espace par un idéogramme (U+678F)
            // — décalage d'index entre le modèle tiny et son dictionnaire. Rien à voir
            // avec un caractère lu : sur un RIB français il n'y a pas d'idéogrammes.
            let text = text.replace('\u{678F}', " ");
            let text = text.as_str();
            let Some(line_rect) = bounding(&region.bounding_box.points) else {
                continue;
            };

            let words: Vec<String> = text
                .split_whitespace()
                .flat_map(split_digit_letter)
                .collect();
            if words.is_empty() {
                continue;
            }

            // Une boîte par mot quand PP-OCR les fournit et que le découpage n'a pas
            // changé le nombre de mots ; sinon répartition de la ligne entre les mots,
            // au prorata de leur longueur.
            let word_rects: Vec<Rect> = match &region.word_boxes {
                Some(boxes) if boxes.len() == words.len() => {
                    boxes.iter().filter_map(|b| bounding(&b.points)).collect()
                }
                _ => {
                    let total: usize = words.iter().map(|w| w.chars().count()).sum::<usize>()
                        + words.len().saturating_sub(1);
                    let unit = line_rect.width() as f32 / total.max(1) as f32;
                    let mut x = line_rect.left() as f32;
                    words
                        .iter()
                        .map(|w| {
                            let width = unit * w.chars().count() as f32;
                            let r = Rect::from_tlhw(
                                line_rect.top(),
                                x.round() as i32,
                                line_rect.height(),
                                width.round().max(1.0) as i32,
                            );
                            x += width + unit;
                            r
                        })
                        .collect()
                }
            };

            if word_rects.len() != words.len() {
                continue;
            }

            let mut chars: Vec<TextChar> = Vec::new();
            for (i, (word, rect)) in words.iter().zip(word_rects.iter()).enumerate() {
                let word: &str = word;
                if i > 0 {
                    // l'espace porte une boîte vide entre deux mots : c'est le séparateur
                    // que `TextLine::words` attend
                    let prev = chars.last().map(|c| c.rect.right()).unwrap_or(rect.left());
                    chars.push(TextChar {
                        char: ' ',
                        rect: Rect::from_tlhw(
                            rect.top(),
                            prev,
                            rect.height(),
                            (rect.left() - prev).max(1),
                        ),
                    });
                }
                chars.extend(spread(word, *rect));
            }

            if !chars.is_empty() {
                lines.push(TextLine::new(chars));
            }
        }
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_glued_to_letters_are_split() {
        assert_eq!(split_digit_letter("222AVENUE"), vec!["222", "AVENUE"]);
        assert_eq!(split_digit_letter("44800ST"), vec!["44800", "ST"]);
        assert_eq!(split_digit_letter("RUE"), vec!["RUE"]);
        assert_eq!(split_digit_letter("44100"), vec!["44100"]);
    }

    /// Un identifiant reste entier, où que soit le chiffre : ce n'est pas une frontière.
    #[test]
    fn identifiers_stay_whole() {
        assert_eq!(split_digit_letter("FR76"), vec!["FR76"]);
        assert_eq!(split_digit_letter("CEPAFRPP444"), vec!["CEPAFRPP444"]);
        assert_eq!(split_digit_letter("CMCIFR2A"), vec!["CMCIFR2A"]);
        assert_eq!(split_digit_letter("FTNOFRP1XXX"), vec!["FTNOFRP1XXX"]);
        // un chiffre seul en tête n'est pas un numéro : « 1er » reste entier
        assert_eq!(split_digit_letter("1er"), vec!["1er"]);
    }
}
