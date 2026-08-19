use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageReader};
use log::trace;
use regex::Regex;

use crate::{
    image_utils::{clean_image, only_rotate, resize, rotate, rotate_rect, save_image_in_debug},
    lines::{extract_anchors, TextLine},
    ppocr::{image_to_string, recognize_anchors},
    provenance::{AnchorSource, Engine, Provenance, TextStats},
    rib::{extract_fr_bic, extract_iban, Rib},
    shapes::{Anchor, Point},
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

    let (page_text, text_lines, maybe_anchors) = recognize_anchors(img, &iban_regex, None);
    let maybe_anchor = maybe_anchors.first();

    // empreinte de forme du texte de la première passe : des comptes, jamais le texte
    if provenance.page_text_stats.is_none() {
        provenance.page_text_stats = Some(TextStats::of(&page_text));
    }

    if let Some(anchor) = maybe_anchor {
        provenance.anchor = Some(AnchorSource::PpOcr);
        provenance.anchor_height = Some(anchor.height);
    }

    if let Some(iban) = extract_iban(&page_text) {
        trace!("early returns from page text for: {}", name);

        provenance.engine = Some(Engine::PpOcrPage);

        let bic = extract_fr_bic(&page_text, Some(&iban));
        let account_holder =
            zoom_and_extract_account_holder_traced(img, text_lines, name, provenance);

        return Some(Rib::from_iban(iban, account_holder, bic));
    };

    if let Some(anchor) = maybe_anchor {
        trace!("ppocr anchor found");

        let iban_image = crop(img, anchor.iban_mask(), name, "mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::PpOcrCrop);

            let account_holder =
                zoom_and_extract_account_holder_traced(img, text_lines.clone(), name, provenance);
            let bic = extract_fr_bic(&page_text, Some(&iban));

            return Some(Rib::from_iban(iban, account_holder, bic));
        }

        // maybe this is a long iban with some | between words
        let iban_image = crop(img, anchor.narrow_iban_mask(), name, "narrow_mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::PpOcrNarrowCrop);

            let account_holder =
                zoom_and_extract_account_holder_traced(img, text_lines, name, provenance);
            let bic = extract_fr_bic(&page_text, Some(&iban));

            return Some(Rib::from_iban(iban, account_holder, bic));
        }
    }

    let (_hocr_string, maybe_angle, maybe_anchor) = tess_analyze(img);

    if let Some(angle) = maybe_angle {
        provenance.angle_deg = Some(angle.to_degrees());
    }

    // Après rotation, l'ancre était redétectée par une seconde analyse hocr de la page
    // entière — quatre à cinq secondes pour une position qui se calcule : la rotation
    // d'un rectangle est de la géométrie. La seconde analyse ne reste que lorsqu'il n'y
    // avait pas d'ancre avant rotation, auquel cas elle cherche et ne redécouvre pas.
    let (img, maybe_anchor) = maybe_angle
        .map(|angle| {
            let rotated_img = rotate(img, angle);
            let new_anchor = match &maybe_anchor {
                Some(anchor) => {
                    let (x, y, w, h) = rotate_rect(
                        (
                            anchor.top_left.x,
                            anchor.top_left.y,
                            anchor.width,
                            anchor.height,
                        ),
                        img.width(),
                        img.height(),
                        angle,
                    );
                    Some(Anchor::new(Point::new(x, y), Point::new(x + w, y + h)))
                }
                None => tess_analyze(&rotated_img).2,
            };
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

            let (page_text, text_lines, _) = recognize_anchors(&img, &iban_regex, None);
            let account_holder =
                zoom_and_extract_account_holder_traced(&img, text_lines, name, provenance);
            let bic = extract_fr_bic(&page_text, Some(&iban));

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

/// Texte des lignes déjà reconnues dont la boîte tombe dans le masque, dans l'ordre
/// vertical, avec les mots de chaque ligne triés de gauche à droite.
///
/// Sert à trier les candidats avant de payer un recadrage : la page a été reconnue une
/// fois, et son texte suffit à dire si un voisinage ressemble à une domiciliation ou à
/// un titulaire. Il ne remplace pas le recadrage pour la lecture elle-même.
fn text_in_mask(text_lines: &[TextLine], (x, y, w, h): (u32, u32, u32, u32)) -> String {
    let (left, top, right, bottom) = (x as i32, y as i32, (x + w) as i32, (y + h) as i32);

    let mut rows: Vec<(i32, String)> = text_lines
        .iter()
        .filter_map(|line| {
            let mut words: Vec<(i32, String)> = line
                .words()
                .filter(|word| {
                    let r = word.bounding_rect();
                    let cx = (r.left() + r.right()) / 2;
                    let cy = (r.top() + r.bottom()) / 2;
                    cx >= left && cx < right && cy >= top && cy < bottom
                })
                .map(|word| (word.bounding_rect().left(), word.to_string()))
                .collect();

            if words.is_empty() {
                return None;
            }
            words.sort_by_key(|(x, _)| *x);

            let text = words
                .into_iter()
                .map(|(_, w)| w)
                .collect::<Vec<String>>()
                .join(" ");
            Some((line.bounding_rect().top(), text))
        })
        .collect();

    rows.sort_by_key(|(top, _)| *top);
    rows.into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<String>>()
        .join("\n")
}

fn zoom_and_extract_account_holder_traced(
    img: &DynamicImage,
    text_lines: Vec<TextLine>,
    name: &str,
    provenance: &mut Provenance,
) -> Option<String> {
    // Le code postal peut être collé à la ville — « 44800ST HERBLAIN » — selon le
    // moteur et l'image ; l'espace n'est pas garanti. Le chemin texte le tolère déjà,
    // le chemin image l'exigeait, et perdait alors toute ancre de titulaire.
    let code_postal_line_regex = Regex::new(r"[[:space:]]*\d{5}\s*[[:alpha:]]").unwrap();
    let code_postal_word_regex = Regex::new(r"^\d{5}").unwrap();

    let postal_anchors = extract_anchors(
        text_lines.clone(),
        &code_postal_word_regex,
        Some(&code_postal_line_regex),
    );

    // Un bloc adressé n'est pas forcément le titulaire : l'agence de domiciliation a
    // elle aussi un code postal. Le chemin texte s'en protège en classant chaque bloc
    // par son contexte ; ici, rien ne le faisait — et le recadrage aligné à droite,
    // tenté faute de civilité, décale la fenêtre vers la gauche jusqu'à happer le nom du
    // titulaire voisin, qui se retrouve collé à l'adresse de l'agence.
    let domiciliation = Regex::new(r"(?i)(domiciliation|agence|cadre r[ée]serv[ée])").unwrap();
    let holder_label = Regex::new(r"(?i)(titulaire|intitul[ée]|account owner)").unwrap();

    // Le recadrage est un vrai zoom : sur les photos, la reconnaissance rapprochée lit
    // les petits caractères que la passe pleine page rate. Lire le bloc dans les lignes
    // de la page au lieu de recadrer a été mesuré — moins vingt points de titulaire.
    let read_mask = |index: usize, mask: (u32, u32, u32, u32), suffix: &str| -> String {
        let cropped_img = crop(img, mask, name, &format!("{}_{}", index, suffix));
        image_to_string(cropped_img)
    };

    // Chaque code postal détecté coûtait jusqu'à deux recadrages reconnus — jusqu'à
    // douze appels OCR sur un document dense en codes postaux (agence, mentions,
    // cachet), pour un seul bloc utile. La lecture pleine page, déjà en main, permet de
    // trier avant de payer : les codes postaux dont le voisinage se présente comme une
    // domiciliation sont écartés d'emblée, ceux dont le voisinage porte une civilité ou
    // un libellé de titulaire passent en premier, et l'examen s'arrête au premier bloc
    // convaincant.
    provenance.postal_anchors = postal_anchors.len() as u32;

    let mut ranked: Vec<(usize, &Anchor, i32)> = postal_anchors
        .iter()
        .enumerate()
        .map(|(index, anchor)| {
            let around = text_in_mask(&text_lines, anchor.addr_mask());
            let score = if holder_label.is_match(&around) {
                2
            } else if match_civilite(&around) {
                1
            } else if domiciliation.is_match(&around) {
                -1
            } else {
                0
            };
            (index, anchor, score)
        })
        .filter(|(_, _, score)| *score >= 0)
        .collect();
    ranked.sort_by_key(|(_, _, score)| -*score);
    provenance.holder_candidates = ranked.len() as u32;

    let mut account_holders: Vec<String> = Vec::new();
    for (index, anchor, score) in ranked {
        provenance.holder_blocks_read += 1;
        let text = read_mask(index, anchor.addr_mask(), "addr_mask");
        // un bloc qui se présente comme domiciliation ne devient pas titulaire, même
        // en le recadrant autrement
        if domiciliation.is_match(&text) && !holder_label.is_match(&text) {
            continue;
        }
        let text = if match_civilite(&text) {
            Some(text)
        } else {
            let new_text = read_mask(
                index,
                anchor.right_align_addr_mask(),
                "right_align_addr_mask",
            );
            if match_civilite(&new_text) && !domiciliation.is_match(&new_text) {
                Some(new_text)
            } else {
                None
            }
        };
        if let Some(holder) = text.and_then(|t| trim_holder(&t, &code_postal_line_regex)) {
            let labelled = holder_label.is_match(&holder);
            account_holders.push(holder);
            // le premier bloc porteur d'un libellé — ou le premier tout court quand la
            // page n'en désigne aucun — suffit : inutile de reconnaître les suivants
            if labelled || score >= 1 {
                break;
            }
        }
    }
    // à plusieurs candidats, celui qui porte un libellé de titulaire l'emporte
    account_holders.sort_by_key(|text| !holder_label.is_match(text));

    // `s` porte déjà ses sauts de ligne : les recollecter caractère à caractère
    // aplatirait le titulaire en une seule ligne
    let account_holder = account_holders.first().cloned();

    if account_holder.is_some() {
        return account_holder;
    }

    // Le mot « titulaire » se cherche dans les lignes déjà reconnues : relancer la
    // reconnaissance de la page entière pour l'y trouver coûtait un appel complet.
    let account_holder_word_regex = Regex::new(r"(?i)titulaire").unwrap();
    let account_holder_anchors = extract_anchors(text_lines, &account_holder_word_regex, None);

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
            image_to_string(cropped_img)
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

/// Lit l'IBAN dans un recadrage : PP-OCR d'abord, tesseract en repli.
///
/// PP-OCR lit ces recadrages plus souvent et bien plus vite que tesseract. Le repli
/// reste : mesuré sans lui, l'IBAN des photos perd trois documents sur trente-deux,
/// et un second modèle PP-OCR à sa place n'en récupère qu'un — deux tailles d'un même
/// modèle partagent leurs erreurs.
fn extract_iban_in_image(cropped_img: &DynamicImage, name: &str) -> Option<String> {
    let ocr = image_to_string(cropped_img.clone());
    if let Some(iban) = extract_iban(&ocr) {
        return Some(iban);
    }

    let tess = img_to_string_using_tesseract(cropped_img.clone());
    if let Some(iban) = extract_iban(&tess) {
        return Some(iban);
    }

    log::trace!("not found for {}: {} / {}", name, ocr, tess);

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
