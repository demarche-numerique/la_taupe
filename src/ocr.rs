use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageReader};
use log::trace;
use ocrs::TextLine;
use regex::Regex;

use crate::{
    image_utils::{clean_image, only_rotate, resize, rotate, save_image_in_debug},
    ocrs::{extract_anchors, image_to_string_using_ocrs, ocrs_anchors},
    provenance::{AnchorSource, Engine, Provenance},
    rib::{extract_fr_bic, extract_iban, Rib},
    tesseract::{img_to_string_using_tesseract, tess_analyze},
    text::simple_account_holder::find_simple_account_holder,
};

const OPTIMAL_TESSERACT_HEIGHT: u32 = 30;

pub fn image_bytes_to_rib(content: Vec<u8>, name: &str) -> Option<Rib> {
    image_bytes_to_rib_traced(content, name, &mut Provenance::default())
}

/// Même traitement, en consignant au passage la stratégie qui a abouti.
pub fn image_bytes_to_rib_traced(
    content: Vec<u8>,
    name: &str,
    provenance: &mut Provenance,
) -> Option<Rib> {
    let img = bytes_to_img(content)?;

    provenance.image_width = img.width();
    provenance.image_height = img.height();

    save_image_in_debug(&img, name, "");

    if let Some(rib) = zoom_and_extract(&img, name, provenance) {
        return Some(rib);
    }

    provenance.second_pass = true;

    let cleaned_img = clean_image(&img, name);
    zoom_and_extract(&cleaned_img, name, provenance)
}

pub fn zoom_and_extract(
    img: &DynamicImage,
    name: &str,
    provenance: &mut Provenance,
) -> Option<Rib> {
    let iban_regex = Regex::new(r"(?:^|\s)FR[\dO]").unwrap();

    let (ocrs_text, text_lines, maybe_anchors) = ocrs_anchors(img, &iban_regex, None);
    let maybe_anchor = maybe_anchors.first();

    if let Some(anchor) = maybe_anchor {
        provenance.anchor = Some(AnchorSource::Ocrs);
        provenance.anchor_height = Some(anchor.height);
    }

    if let Some(iban) = extract_iban(&ocrs_text) {
        trace!("early returns from ocrs for: {}", name);

        provenance.engine = Some(Engine::OcrsPage);

        let bic = extract_fr_bic(&ocrs_text);
        let account_holder = zoom_and_extract_account_holder(img, text_lines, name);

        return Some(Rib::from_iban(iban, account_holder, bic));
    };

    if let Some(anchor) = maybe_anchor {
        trace!("ocrs anchor found");

        let iban_image = crop(img, anchor.iban_mask(), name, "mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::OcrsCrop);

            let account_holder = zoom_and_extract_account_holder(img, text_lines.clone(), name);
            let bic = extract_fr_bic(&ocrs_text);

            return Some(Rib::from_iban(iban, account_holder, bic));
        }

        // maybe this is a long iban with some | between words
        let iban_image = crop(img, anchor.narrow_iban_mask(), name, "narrow_mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::OcrsNarrowCrop);

            let account_holder = zoom_and_extract_account_holder(img, text_lines, name);
            let bic = extract_fr_bic(&ocrs_text);

            return Some(Rib::from_iban(iban, account_holder, bic));
        }
    }

    let (_hocr_string, maybe_angle, maybe_anchor) = tess_analyze(img);

    if let Some(angle) = maybe_angle {
        provenance.angle_deg = Some(angle.to_degrees());
    }

    let (img, maybe_anchor) = maybe_angle
        .map(|angle| {
            let rotated_img = rotate(img, angle);
            let (_, _, new_anchor) = tess_analyze(&rotated_img);
            (rotated_img, new_anchor)
        })
        .unwrap_or((img.clone(), maybe_anchor));

    if let Some(anchor) = maybe_anchor {
        trace!("tess anchor found");

        if provenance.anchor.is_none() {
            provenance.anchor = Some(AnchorSource::Tesseract);
            provenance.anchor_height = Some(anchor.height);
        }

        let iban_image = crop(&img, anchor.iban_mask(), name, "mask");

        let iban_image = only_rotate(&iban_image, name);
        let iban_image = resize(&iban_image, anchor.height, OPTIMAL_TESSERACT_HEIGHT);
        save_image_in_debug(&iban_image, name, "rotated_resized_mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::TessCrop);

            let (ocrs_text, text_lines, _) = ocrs_anchors(&img, &iban_regex, None);
            let account_holder = zoom_and_extract_account_holder(&img, text_lines, name);
            let bic = extract_fr_bic(&ocrs_text);

            return Some(Rib::from_iban(iban, account_holder, bic));
        }
    }

    None
}

fn match_civilite(s: &str) -> bool {
    find_civilite(s).is_some()
}

fn find_civilite(s: &str) -> Option<usize> {
    let civilite =
        Regex::new(r"(?i)(^|\s)(m|monsieur|mr|mademoiselle|ml|mle|mlle|melle|madame|mme)\.?\s")
            .unwrap();
    let prenom_nom_ou =
        Regex::new(r"[[:upper:]]+ +[[:upper:]]+ +OU +[[:upper:]]+ +[[:upper:]]+").unwrap();

    civilite
        .find(s)
        .or_else(|| prenom_nom_ou.find(s))
        .map(|m| m.start())
}

fn zoom_and_extract_account_holder(
    img: &DynamicImage,
    text_lines: Vec<TextLine>,
    name: &str,
) -> Option<String> {
    let code_postal_line_regex = Regex::new(r"[[:space:]]*\d{5}\s+[[:alpha:]]").unwrap();
    let code_postal_word_regex = Regex::new(r"^\d{5}").unwrap();

    let postal_anchors = extract_anchors(
        text_lines,
        &code_postal_word_regex,
        Some(&code_postal_line_regex),
    );

    let account_holders = postal_anchors
        .iter()
        .enumerate()
        .map(|(index, anchor)| {
            let cropped_img = crop(
                img,
                anchor.addr_mask(),
                name,
                &format!(r#"{}_addr_mask"#, index),
            );
            (index, image_to_string_using_ocrs(cropped_img), anchor)
        })
        .filter_map(|(index, text, anchor)| {
            if match_civilite(&text) {
                Some(text)
            } else {
                let cropped_img = crop(
                    img,
                    anchor.right_align_addr_mask(),
                    name,
                    &format!(r#"{}_right_align_addr_mask"#, index),
                );
                let new_text = image_to_string_using_ocrs(cropped_img);

                if match_civilite(&new_text) {
                    Some(new_text)
                } else {
                    None
                }
            }
        })
        .filter_map(|text| trim_holder(&text, &code_postal_line_regex))
        .collect::<Vec<String>>();

    // `s` porte déjà ses sauts de ligne : les recollecter caractère à caractère
    // aplatirait le titulaire en une seule ligne
    let account_holder = account_holders.first().cloned();

    if account_holder.is_some() {
        return account_holder;
    }

    let account_holder_word_regex = Regex::new(r"(?i)titulaire").unwrap();
    let (_, _text_lines, account_holder_anchors) =
        ocrs_anchors(img, &account_holder_word_regex, None);

    account_holder_anchors
        .iter()
        .enumerate()
        .map(|(index, anchor)| {
            let cropped_img = crop(
                img,
                anchor.account_holder_mask(),
                name,
                &format!(r#"{}_account_holder_mask"#, index),
            );
            image_to_string_using_ocrs(cropped_img)
        })
        .filter(|text| account_holder_word_regex.is_match(text))
        .find_map(|text| find_simple_account_holder(&text, 1))
}

/// Restreint un bloc reconnu au titulaire : on écarte ce qui précède la civilité, et ce
/// qui suit le code postal.
///
/// Le code postal peut manquer alors qu'une civilité est présente — le recadrage aligné à
/// droite décale la fenêtre et peut le laisser hors champ, et l'OCR ne le restitue pas
/// toujours sous une forme reconnaissable. Dans ce cas on conserve le bloc, borné par la
/// hauteur du recadrage, plutôt que d'abandonner.
fn trim_holder(text: &str, postal_code: &Regex) -> Option<String> {
    let start = find_civilite(text)?;
    let text = text[start..].trim();

    let lines: Vec<&str> = text.lines().collect();
    let end = lines
        .iter()
        .position(|line| postal_code.is_match(line))
        .map_or(lines.len(), |index| index + 1);

    Some(lines[..end].join("\n"))
}

fn crop(
    img: &DynamicImage,
    (x, y, width, height): (u32, u32, u32, u32),
    name: &str,
    suffix: &str,
) -> DynamicImage {
    let result = img.crop_imm(x, y, width, height);
    save_image_in_debug(&result, name, suffix);
    result
}

fn bytes_to_img(content: Vec<u8>) -> Option<DynamicImage> {
    let mut decoder = ImageReader::new(Cursor::new(content))
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;

    let orientation = decoder.orientation().ok()?;
    let mut img = DynamicImage::from_decoder(decoder).ok()?;
    img.apply_orientation(orientation);
    Some(img.into_luma8().into())
}

fn extract_iban_in_image(cropped_img: &DynamicImage, name: &str) -> Option<String> {
    let tess_iban = img_to_string_using_tesseract(cropped_img.clone());
    if let Some(iban) = extract_iban(&tess_iban) {
        return Some(iban);
    };

    let ocrs_iban = image_to_string_using_ocrs(cropped_img.clone());
    if let Some(iban) = extract_iban(&ocrs_iban) {
        return Some(iban);
    };

    log::trace!(
        "not found for {}: tess_iban: {}, ocrs_iban: {}",
        name,
        tess_iban,
        ocrs_iban
    );

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn postal_code() -> Regex {
        Regex::new(r"[[:space:]]*\d{5}\s+[[:alpha:]]").unwrap()
    }

    #[test]
    fn holder_is_trimmed_around_civility_and_postal_code() {
        let text =
            "Titulaire du compte\nM MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES\nDomiciliation";

        assert_eq!(
            trim_holder(text, &postal_code()).as_deref(),
            Some("M MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES")
        );
    }

    /// Un bloc sans code postal reconnaissable ne doit pas faire tomber l'analyse : le
    /// recadrage aligné à droite peut le laisser hors champ, et l'OCR ne le restitue pas
    /// toujours espacé comme attendu.
    #[test]
    fn a_holder_without_postal_code_is_kept_whole() {
        assert_eq!(
            trim_holder("M MATISSE HENRI\n51 RUE BERNARD ROY", &postal_code()).as_deref(),
            Some("M MATISSE HENRI\n51 RUE BERNARD ROY")
        );

        // code postal collé à la ville : le motif ne le reconnaît pas
        assert_eq!(
            trim_holder("MME KAHLO FRIDA\n44100NANTES", &postal_code()).as_deref(),
            Some("MME KAHLO FRIDA\n44100NANTES")
        );
    }

    #[test]
    fn a_block_without_civility_is_discarded() {
        assert_eq!(
            trim_holder("51 RUE BERNARD ROY\n44100 NANTES", &postal_code()),
            None
        );
    }

    #[test]
    fn couples_without_civility_are_recognised() {
        assert!(match_civilite("HENRI MATISSE OU FRIDA KAHLO"));
        assert!(match_civilite("Madame Kahlo Frida"));
        assert!(!match_civilite("51 RUE BERNARD ROY"));
    }
}
