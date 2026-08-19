use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageReader};
use log::trace;
use regex::Regex;

use crate::{
    image_utils::{clean_image, only_rotate, resize, rotate, rotate_rect, save_image_in_debug},
    lines::{extract_anchors, TextLine},
    ppocr::{image_to_string, recognize_anchors},
    provenance::{AnchorSource, Engine, Provenance, TextStats},
    rib::{extract_fr_bic, extract_iban, join_cell_letters, Rib},
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

    // Empreinte de forme du texte de la page : des comptes, jamais le texte. On garde la
    // lecture la plus fournie — la seconde passe, sur image nettoyée, peut lire ce que
    // la première n'a pas vu, et le drapeau « illisible » ne doit porter que sur ce que
    // le prétraitement n'a pas su rattraper.
    let stats = TextStats::of(&page_text);
    let richer = provenance
        .page_text_stats
        .as_ref()
        .is_none_or(|prev| stats.alphas + stats.digits > prev.alphas + prev.digits);
    if richer {
        provenance.page_text_stats = Some(stats);
    }

    if let Some(anchor) = maybe_anchor {
        provenance.anchor = Some(AnchorSource::PpOcr);
        provenance.anchor_height = Some(anchor.height);
    }

    if let Some(iban) = extract_iban(&page_text) {
        trace!("early returns from page text for: {}", name);

        provenance.engine = Some(Engine::PpOcrPage);

        let bic = extract_bic(img, &page_text, &text_lines, &iban, name);
        let account_holder =
            zoom_and_extract_account_holder_traced(img, text_lines, name, provenance);

        return Some(Rib::from_iban(iban, account_holder, bic));
    };

    if let Some(anchor) = maybe_anchor {
        trace!("ppocr anchor found");

        let iban_image = crop(img, anchor.iban_mask(), name, "mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::PpOcrCrop);

            let bic = extract_bic(img, &page_text, &text_lines, &iban, name);
            let account_holder =
                zoom_and_extract_account_holder_traced(img, text_lines.clone(), name, provenance);

            return Some(Rib::from_iban(iban, account_holder, bic));
        }

        // maybe this is a long iban with some | between words
        let iban_image = crop(img, anchor.narrow_iban_mask(), name, "narrow_mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::PpOcrNarrowCrop);

            let bic = extract_bic(img, &page_text, &text_lines, &iban, name);
            let account_holder =
                zoom_and_extract_account_holder_traced(img, text_lines, name, provenance);

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
            let bic = extract_bic(&img, &page_text, &text_lines, &iban, name);
            let account_holder =
                zoom_and_extract_account_holder_traced(&img, text_lines, name, provenance);

            return Some(Rib::from_iban(iban, account_holder, bic));
        }
    }

    None
}

fn match_civilite(s: &str) -> bool {
    find_civilite(s).is_some()
}

/// Position du début du bloc titulaire quand un libellé le désigne : ce qui suit le
/// libellé sur sa ligne (« Titulaire : M … »), sinon la ligne suivante (« Nom et
/// adresse du bénéficiaire » en ligne à part).
fn after_holder_label(s: &str) -> Option<usize> {
    let label = Regex::new(
        r"(?i)(nom et adresse du )?(titulaire|intitul[ée]|account owner|b[ée]n[ée]ficiaire)s?( du compte| de compte| du client)?\s*(n[°o]\s*)?[:.\-]*",
    )
    .unwrap();
    let m = label.find(s)?;

    let line_end = s[m.end()..]
        .find('\n')
        .map(|i| m.end() + i)
        .unwrap_or(s.len());

    let rest = s[m.end()..line_end].trim();
    if rest.is_empty() {
        (line_end < s.len()).then_some(line_end + 1)
    } else {
        Some(line_end - (s[m.end()..line_end].trim_start().len()))
    }
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

/// Lit le BIC : par motif dans le texte de la page, d'abord tel quel, puis les
/// cellules recollées ; enfin, faute de candidat, en recadrant autour du libellé.
///
/// Le BIC n'avait qu'une regex sur la première lecture. Or sur les documents réels il
/// est souvent imprimé dans un tableau à une lettre par cellule : l'OCR rend les
/// cellules comme des mots courts — « Ps sƫ FRP P NƫE » — et la regex ne voit jamais
/// la suite entière. La lecture pleine page est pourtant la bonne : recadrer la zone
/// fait perdre le contexte et les moteurs n'y voient que des traits. Il suffit de
/// recoller les cellules avant d'appliquer le motif.
fn extract_bic(
    img: &DynamicImage,
    page_text: &str,
    text_lines: &[TextLine],
    iban: &str,
    name: &str,
) -> Option<String> {
    if let Some(bic) = extract_fr_bic(page_text, Some(iban)) {
        return Some(bic);
    }

    if let Some(bic) = extract_fr_bic(&join_cell_letters(page_text), Some(iban)) {
        trace!("BIC par recollement des cellules");
        return Some(bic);
    }

    // Sur d'autres documents, l'OCR rend chaque cellule comme une ligne à part entière,
    // côte à côte à la même hauteur : ligne par ligne, rien à recoller. On les réunit
    // par la géométrie — même bande verticale, ordre horizontal — avant le motif.
    let rows = join_cell_lines(text_lines);
    if let Some(bic) = extract_fr_bic(&rows, Some(iban)) {
        trace!("BIC par recollement géométrique des cellules");
        return Some(bic);
    }

    let bic_word = Regex::new(r"(?i)^bic\b").unwrap();
    let anchors = extract_anchors(text_lines.to_vec(), &bic_word, None);

    for (index, anchor) in anchors.iter().enumerate().take(2) {
        let cropped = crop(img, anchor.bic_mask(), name, &format!("{}_bic_mask", index));
        let text = join_cell_letters(&image_to_string(cropped));
        if let Some(bic) = extract_fr_bic(&text, Some(iban)) {
            trace!("BIC par recadrage sur le libellé");
            return Some(bic);
        }
    }

    None
}

/// Réunit en lignes les fragments reconnus séparément mais alignés à la même hauteur —
/// typiquement les cellules d'un tableau, une lettre chacune, que le détecteur rend
/// comme autant de lignes. Rend un texte où chaque bande verticale est une ligne, les
/// fragments joints sans espace quand ils sont courts et contigus, par un espace sinon.
fn join_cell_lines(text_lines: &[TextLine]) -> String {
    let mut items: Vec<(i32, i32, i32, String)> = text_lines
        .iter()
        .map(|l| {
            let r = l.bounding_rect();
            (r.top(), r.left(), r.height().max(1), l.to_string())
        })
        .collect();
    items.sort_by_key(|(top, left, _, _)| (*top, *left));

    let mut rows: Vec<Vec<(i32, i32, String)>> = Vec::new();
    let mut row_top: i32 = i32::MIN;
    let mut row_h: i32 = 1;
    for (top, left, h, text) in items {
        let same_band = (top - row_top).abs() < row_h.max(h) / 2;
        if same_band {
            rows.last_mut().unwrap().push((left, h, text));
        } else {
            rows.push(vec![(left, h, text)]);
            row_top = top;
            row_h = h;
        }
    }

    rows.into_iter()
        .map(|mut row| {
            row.sort_by_key(|(left, _, _)| *left);
            let mut out = String::new();
            let mut prev_end: Option<i32> = None;
            for (left, h, text) in row {
                let short = text.chars().count() <= 3;
                // contigu : l'écart est inférieur à une hauteur de ligne
                let contiguous = prev_end.is_some_and(|e| left - e < h);
                let glue = short && contiguous;
                if !out.is_empty() && !glue {
                    out.push(' ');
                }
                out.push_str(&text);
                // largeur approchée : une hauteur par caractère
                prev_end = Some(left + text.chars().count() as i32 * h);
            }
            out
        })
        .collect::<Vec<String>>()
        .join("\n")
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
    // « bénéficiaire » : certains RIB étiquettent le bloc « Nom et adresse du
    // bénéficiaire » — même rôle que « titulaire », mesuré sur les corpus réels
    let holder_label =
        Regex::new(r"(?i)(titulaire|intitul[ée]|account owner|b[ée]n[ée]ficiaire)").unwrap();

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
    // « bénéficiaire » n'entre pas dans ce repli : mesuré, le masque à compte de lignes
    // fixe rend des blocs tronqués sur ces mises en page — pire que rien
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

/// Restreint un bloc reconnu au titulaire : on écarte ce qui précède la civilité — ou le
/// libellé (« titulaire », « bénéficiaire »…) quand la civilité manque —, et ce qui suit
/// le code postal.
///
/// Le libellé fait ancre à part entière : un bloc « Nom et adresse du bénéficiaire »
/// n'a souvent pas de civilité, et l'exiger perdait le titulaire alors que le document
/// le désigne explicitement. Ce qui suit le libellé — sur sa ligne, sinon les lignes
/// d'en dessous — est le bloc.
///
/// Le code postal peut manquer alors qu'une civilité est présente — le recadrage aligné à
/// droite décale la fenêtre et peut le laisser hors champ, et l'OCR ne le restitue pas
/// toujours sous une forme reconnaissable. Dans ce cas on conserve le bloc, borné par la
/// hauteur du recadrage, plutôt que d'abandonner.
fn trim_holder(text: &str, postal_code: &Regex) -> Option<String> {
    // Une civilité avant le libellé est un faux positif — un « M » isolé dans le texte
    // de banque au-dessus suffit — et ferait déborder le bloc vers le haut : le libellé
    // prime alors. Après le libellé, la civilité est dans le bloc : elle reste l'ancre,
    // au plus près du nom.
    let start = match (find_civilite(text), after_holder_label(text)) {
        (Some(civility), Some(label)) => Some(civility.max(label)),
        (civility, label) => civility.or(label),
    }?;
    let text = text[start..].trim();

    if text.is_empty() {
        return None;
    }

    let lines: Vec<&str> = text.lines().collect();
    let postal = lines.iter().position(|line| postal_code.is_match(line));
    let end = postal.map_or(lines.len(), |index| index + 1);

    // Ancré sur le seul libellé — sans civilité pour confirmer que c'est bien un nom —
    // un bloc qui ne va pas jusqu'à son code postal est douteux : probablement coupé,
    // ou pas un titulaire. Ne rien rendre plutôt qu'un bloc douteux.
    if !match_civilite(text) && postal.is_none() {
        return None;
    }

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
    fn a_block_without_civility_nor_label_is_discarded() {
        assert_eq!(
            trim_holder("51 RUE BERNARD ROY\n44100 NANTES", &postal_code()),
            None
        );
    }

    /// Un libellé fait ancre même sans civilité : « Nom et adresse du bénéficiaire »
    /// désigne le bloc, l'exiger perdait des titulaires explicitement étiquetés.
    #[test]
    fn a_labelled_block_without_civility_is_kept() {
        let text = "Nom et adresse du bénéficiaire\nMATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES\nDomiciliation";
        assert_eq!(
            trim_holder(text, &postal_code()).as_deref(),
            Some("MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES")
        );

        // libellé et titulaire sur la même ligne
        assert_eq!(
            trim_holder("Titulaire : MATISSE HENRI\n44100 NANTES", &postal_code()).as_deref(),
            Some("MATISSE HENRI\n44100 NANTES")
        );

        // un libellé qui ne désigne rien ne rend rien
        assert_eq!(trim_holder("Titulaire du compte", &postal_code()), None);

        // sans civilité ni code postal pour le fermer, le bloc est douteux :
        // probablement coupé, ou pas un titulaire — ne rien rendre
        assert_eq!(
            trim_holder(
                "Nom et adresse du bénéficiaire\nMATISSE HENRI",
                &postal_code()
            ),
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
