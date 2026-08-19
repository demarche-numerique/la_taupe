use std::sync::OnceLock;

use image::DynamicImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem, TextLine};
use regex::Regex;
use rten::Model;

use crate::shapes::{Anchor, Point};

const DETECTION_MODEL: &[u8] = include_bytes!("../models/text-detection.rten");
const RECOGNITION_MODEL: &[u8] = include_bytes!("../models/text-recognition.rten");

/// Moteur partagé.
///
/// Il était auparavant reconstruit à chaque appel, modèles rechargés compris, alors
/// qu'un même document en déclenche plusieurs — un par recadrage d'adresse, et un par
/// orientation essayée.
static ENGINE: OnceLock<OcrEngine> = OnceLock::new();

fn engine() -> &'static OcrEngine {
    ENGINE.get_or_init(|| {
        #[allow(clippy::const_is_empty)]
        if DETECTION_MODEL.is_empty() || RECOGNITION_MODEL.is_empty() {
            panic!("--> ocrs models are empty in models/ directory. Please run `download_models.sh` to download the models.");
        }

        let detection_model = Model::load_static_slice(DETECTION_MODEL).unwrap();
        let recognition_model = Model::load_static_slice(RECOGNITION_MODEL).unwrap();

        OcrEngine::new(OcrEngineParams {
            detection_model: Some(detection_model),
            recognition_model: Some(recognition_model),
            ..Default::default()
        })
        .unwrap()
    })
}

/// Moteur en service, choisi par `LA_TAUPE_OCR_ENGINE` : `ppocr` (défaut) ou `ocrs`.
/// Un seul point de décision, pour que toute la cascade bascule d'un coup et que les
/// deux moteurs se mesurent sur le même binaire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Ocrs,
    PpOcr,
}

pub fn selected_engine() -> Engine {
    static SELECTED: OnceLock<Engine> = OnceLock::new();

    *SELECTED.get_or_init(|| match std::env::var("LA_TAUPE_OCR_ENGINE").as_deref() {
        Ok("ocrs") => Engine::Ocrs,
        _ => Engine::PpOcr,
    })
}

/// Détecte les mots, les groupe en lignes et les reconnaît, avec le moteur en service.
pub fn recognize(img: &DynamicImage) -> Vec<TextLine> {
    match selected_engine() {
        Engine::PpOcr => crate::ppocr::recognize(img),
        Engine::Ocrs => crate::timing::measure(crate::timing::ocrs, || recognize_inner(img)),
    }
}

fn recognize_inner(img: &DynamicImage) -> Vec<TextLine> {
    let img = img.clone().into_rgb8();
    let engine = engine();

    // Apply standard image pre-processing expected by this library (convert
    // to greyscale, map range to [-0.5, 0.5]).
    let img_source = ImageSource::from_bytes(img.as_raw(), img.dimensions()).unwrap();
    let ocr_input = engine.prepare_input(img_source).unwrap();

    // Get oriented bounding boxes of text words in input image.
    let word_rects = engine.detect_words(&ocr_input).unwrap();

    // Group words into lines. Each line is represented by a list of word
    // bounding boxes.
    let line_rects = engine.find_text_lines(&ocr_input, &word_rects);

    engine
        .recognize_text(&ocr_input, &line_rects)
        .unwrap()
        .iter()
        .flatten()
        .cloned()
        .collect()
}

fn lines_to_text(text_lines: &[TextLine]) -> String {
    text_lines
        .iter()
        // Filter likely spurious detections. With future model improvements
        // this should become unnecessary.
        .filter(|l| l.to_string().len() > 1)
        .map(|l| l.to_string())
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn image_to_string_using_ocrs(img: DynamicImage) -> String {
    lines_to_text(&recognize(&img))
}

pub fn ocrs_anchors(
    img: &DynamicImage,
    word_regex: &Regex,
    line_regex: Option<&Regex>,
) -> (String, Vec<TextLine>, Vec<Anchor>) {
    let text_lines = recognize(img);
    let text = lines_to_text(&text_lines);
    let anchors = extract_anchors(text_lines.clone(), word_regex, line_regex);

    (text, text_lines, anchors)
}

pub fn extract_anchors(
    text_lines: Vec<TextLine>,
    word_regex: &Regex,
    line_regex: Option<&Regex>,
) -> Vec<Anchor> {
    text_lines
        .iter()
        .filter(|line| {
            if line_regex.is_none() {
                return true;
            }
            line_regex.unwrap().is_match(&line.to_string())
        })
        .flat_map(|line| line.words())
        .filter(|word| word_regex.is_match(&word.to_string()))
        .map(|word| {
            let [p1, _, p3, _, ..] = word
                .rotated_rect()
                .corners()
                .map(|point| [point.x.round() as u32, point.y.round() as u32]);

            // Les masques sont des multiples de la largeur de l'ancre. Pour un code
            // postal collé à sa ville — « 44800ST » — la largeur du mot entier ferait un
            // masque soixante pour cent trop large, qui empiète sur la colonne voisine :
            // on ne garde que la largeur du motif reconnu. Mais seulement quand le motif
            // est ancré en début de mot et bien plus court que lui : l'ancre d'IBAN
            // « FR76 » suivie du reste doit rester le mot entier, sans quoi le recadrage
            // étroit de l'IBAN devient minuscule et le rate.
            let text = word.to_string();
            let matched = word_regex
                .find(&text)
                .filter(|m| m.start() == 0)
                .map(|m| m.end())
                .unwrap_or(0);
            let total = text.chars().count().max(1);
            let width = p1[0].saturating_sub(p3[0]);
            let is_postal = matched == 5 && text.chars().take(5).all(|c| c.is_ascii_digit());
            let matched_width = if is_postal && matched < total {
                (width as f32 * matched as f32 / total as f32).round() as u32
            } else {
                width
            };

            Anchor::new(
                Point::new(p3[0], p3[1]),
                Point::new(p3[0] + matched_width.max(1), p1[1]),
            )
        })
        .collect::<Vec<Anchor>>()
}
