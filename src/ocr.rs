use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageReader};
use log::trace;
use regex::Regex;

use crate::{
    image_utils::{clean_image, only_rotate, resize, rotate, rotate_rect, save_image_in_debug},
    lines::{extract_anchors, max_line_height, TextLine},
    ppocr::{image_to_string, recognize_anchors},
    provenance::{AnchorSource, BlockOutcome, Engine, HolderBlock, OcrLine, Provenance, TextStats},
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

    // Page lue de haut en bas sans que l'IBAN y soit lu : toute la suite de la cascade —
    // recadrages autour d'ancres hautes de trois cents pixels — tomberait à côté. On la
    // mène sur la page redressée.
    let turned;
    // `turn` : l'image lue est la page tournée de tant de degrés, sens horaire — pour
    // ramener à la page la place des blocs candidats au titulaire
    let (img, page_text, text_lines, maybe_anchors, turn) =
        if extract_iban(&page_text).is_none() && mostly_vertical(&text_lines) {
            match turn_upright(img, &iban_regex) {
                Some((image, text, lines, anchors, turn)) => {
                    turned = image;
                    (&turned, text, lines, anchors, turn)
                }
                None => (img, page_text, text_lines, maybe_anchors, 0),
            }
        } else {
            (img, page_text, text_lines, maybe_anchors, 0)
        };
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
        let (holder_img, holder_lines, holder_turn) =
            match upright(img, &text_lines, &iban, &iban_regex) {
                Some((image, lines, extra)) => (image, lines, (turn + extra) % 360),
                None => (img.clone(), text_lines, turn),
            };
        let account_holder = zoom_and_extract_account_holder_traced(
            &holder_img,
            holder_lines,
            name,
            provenance,
            Some(holder_turn),
        );

        return Some(Rib::from_iban(iban, account_holder, bic));
    };

    if let Some(anchor) = maybe_anchor {
        trace!("ppocr anchor found");

        let iban_image = crop(img, anchor.iban_mask(), name, "mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::PpOcrCrop);

            let bic = extract_bic(img, &page_text, &text_lines, &iban, name);
            let account_holder = zoom_and_extract_account_holder_traced(
                img,
                text_lines.clone(),
                name,
                provenance,
                Some(turn),
            );

            return Some(Rib::from_iban(iban, account_holder, bic));
        }

        // maybe this is a long iban with some | between words
        let iban_image = crop(img, anchor.narrow_iban_mask(), name, "narrow_mask");

        if let Some(iban) = extract_iban_in_image(&iban_image, name) {
            provenance.engine = Some(Engine::PpOcrNarrowCrop);

            let bic = extract_bic(img, &page_text, &text_lines, &iban, name);
            let account_holder = zoom_and_extract_account_holder_traced(
                img,
                text_lines,
                name,
                provenance,
                Some(turn),
            );

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
            // un angle quelconque ne se ramène pas à la page par un quart de tour
            let holder_turn = maybe_angle.is_none_or(|a| a.abs() < 0.02).then_some(turn);
            let account_holder = zoom_and_extract_account_holder_traced(
                &img,
                text_lines,
                name,
                provenance,
                holder_turn,
            );

            return Some(Rib::from_iban(iban, account_holder, bic));
        }
    }

    None
}

/// Libellé de titulaire posé à gauche du bloc d'un code postal, dans la hauteur de ce
/// bloc — le plus proche au-dessus du code postal —, rendu par son haut et son bord droit.
fn side_label(
    text_lines: &[TextLine],
    anchor: &Anchor,
    holder_label: &Regex,
) -> Option<(u32, u32)> {
    let (_, mask_top, _, _) = anchor.addr_mask();
    let block_top = mask_top as i32;
    let block_bottom = anchor.bottom_right.y as i32;
    let postal_left = anchor.top_left.x as i32;

    text_lines
        .iter()
        .filter(|line| holder_label.is_match(&line.to_string()))
        .map(|line| line.bounding_rect())
        .filter(|r| {
            let center = (r.top() + r.bottom()) / 2;
            center >= block_top && center <= block_bottom && r.right() <= postal_left
        })
        .max_by_key(|r| r.top())
        .map(|r| (r.top().max(0) as u32, r.right().max(0) as u32))
}

/// Recadrage du bloc entre la hauteur du libellé et le code postal. Vers la gauche, il
/// va aussi loin que le masque aligné à droite — un nom aligné à droite déborde du code
/// postal — sans passer le libellé.
fn side_mask(anchor: &Anchor, (label_top, label_right): (u32, u32)) -> (u32, u32, u32, u32) {
    let (align_x, _, width, _) = anchor.right_align_addr_mask();
    let x = align_x.max(label_right);
    let right = align_x + width;
    let y = label_top.saturating_sub(anchor.height / 2);
    let bottom = anchor.bottom_right.y + anchor.height / 2;

    (
        x,
        y,
        right.saturating_sub(x).max(1),
        bottom.saturating_sub(y).max(1),
    )
}

/// Vrai si la page se lit de haut en bas : la plupart de ses lignes un peu longues sont
/// plus hautes que larges. C'est une photo prise appareil tourné d'un quart de tour.
fn mostly_vertical(lines: &[TextLine]) -> bool {
    let long: Vec<&TextLine> = lines
        .iter()
        .filter(|l| l.to_string().chars().count() >= 6)
        .collect();
    let vertical = long
        .iter()
        .filter(|l| {
            let r = l.bounding_rect();
            r.height() > r.width()
        })
        .count();

    long.len() >= 3 && vertical * 2 > long.len()
}

/// Redresse d'un quart de tour une page lue de haut en bas, pour le titulaire.
///
/// PP-OCR lit l'IBAN d'une photo pivotée, mais ses lignes sont alors des colonnes :
/// hautes de trois cents pixels, elles font des ancres de code postal démesurées, et
/// tous les masques du bloc adresse — des multiples de leur hauteur — tombent à côté.
/// Le titulaire était perdu sur chaque photo prise appareil tourné, un quart des photos
/// réelles. On relit la page tournée dans un sens, puis dans l'autre, et on garde celui
/// qui redonne le même IBAN en lignes horizontales. Rien n'est relu pour une page droite.
fn upright(
    img: &DynamicImage,
    text_lines: &[TextLine],
    iban: &str,
    iban_regex: &Regex,
) -> Option<(DynamicImage, Vec<TextLine>, u16)> {
    if !mostly_vertical(text_lines) {
        return None;
    }

    [(img.rotate90(), 90), (img.rotate270(), 270)]
        .into_iter()
        .find_map(|(rotated, turn)| {
            let (text, lines, _) = recognize_anchors(&rotated, iban_regex, None);
            let same_iban = extract_iban(&text).as_deref() == Some(iban);

            (same_iban && !mostly_vertical(&lines)).then_some((rotated, lines, turn))
        })
}

/// Page tournée d'un quart de tour : l'image, sa lecture, ses lignes, ses ancres d'IBAN
/// et l'angle du quart de tour, sens horaire.
type TurnedPage = (DynamicImage, String, Vec<TextLine>, Vec<Anchor>, u16);

/// Relit une page pivotée d'un quart de tour dans les deux sens, et garde la lecture la
/// plus probante : celle qui donne un IBAN, sinon celle qui porte le plus d'ancres
/// d'IBAN, puis le plus de vocabulaire de RIB, puis de caractères — le sens tête en bas
/// lit du bruit.
fn turn_upright(img: &DynamicImage, iban_regex: &Regex) -> Option<TurnedPage> {
    [(img.rotate90(), 90), (img.rotate270(), 270)]
        .into_iter()
        .map(|(rotated, turn)| {
            let (text, lines, anchors) = recognize_anchors(&rotated, iban_regex, None);
            (rotated, text, lines, anchors, turn)
        })
        .filter(|(_, _, lines, _, _)| !mostly_vertical(lines))
        .max_by_key(|(_, text, _, anchors, _)| {
            let stats = TextStats::of(text);
            (
                extract_iban(text).is_some(),
                anchors.len(),
                stats.vocabulary_hits,
                stats.alphas + stats.digits,
            )
        })
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
    // Une personne morale n'a pas de civilité : sa forme juridique en tête de ligne joue
    // le même rôle — « ASSOC. LES AMIS DE… », « SARL LES FAUVES ». En majuscules
    // seulement, et sans « SA » : trop court pour ne pas surgir au milieu d'un mot lu.
    let legal_form = Regex::new(
        r"(?m)^\s*(ASSOCIATION|ASSOC\.?|ASS|SASU|SAS|SARL|S\.A\.R\.L\.|EURL|SCI|SCP|SELARL|EARL|GAEC|SCEA|SNC|GIE|SCOP|FONDATION)\s",
    )
    .unwrap();

    civilite
        .find(s)
        .or_else(|| prenom_nom_ou.find(s))
        .map(|m| m.start())
        .or_else(|| {
            legal_form
                .captures(s)
                .and_then(|c| c.get(1))
                .map(|m| m.start())
        })
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

/// Rectangle d'un recadrage en fractions de la page d'origine, l'image lue étant cette
/// page tournée de `turn` degrés dans le sens horaire ; `None` si ce n'est pas un quart
/// de tour. Le redressement d'une faible inclinaison, lui, est négligé : quelques pixels.
fn page_rect(
    img: &DynamicImage,
    (x, y, w, h): (u32, u32, u32, u32),
    turn: Option<u16>,
) -> Option<[f32; 4]> {
    let (width, height) = (img.width().max(1) as f32, img.height().max(1) as f32);
    let (x0, y0) = ((x as f32 / width).min(1.0), (y as f32 / height).min(1.0));
    let x1 = ((x + w) as f32 / width).min(1.0);
    let y1 = ((y + h) as f32 / height).min(1.0);
    let back = |fx: f32, fy: f32| match turn {
        Some(90) => Some((fy, 1.0 - fx)),
        Some(180) => Some((1.0 - fx, 1.0 - fy)),
        Some(270) => Some((1.0 - fy, fx)),
        Some(0) => Some((fx, fy)),
        _ => None,
    };
    let (a, b) = (back(x0, y0)?, back(x1, y1)?);
    Some([a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)])
}

fn zoom_and_extract_account_holder_traced(
    img: &DynamicImage,
    text_lines: Vec<TextLine>,
    name: &str,
    provenance: &mut Provenance,
    turn: Option<u16>,
) -> Option<String> {
    provenance.holder_blocks.clear();
    let block = |mask: (u32, u32, u32, u32), outcome: BlockOutcome| HolderBlock {
        rect: page_rect(img, mask, turn),
        outcome,
    };
    // Le code postal peut être collé à la ville — « 44800ST HERBLAIN » — selon le
    // moteur et l'image ; l'espace n'est pas garanti. Le chemin texte le tolère déjà,
    // le chemin image l'exigeait, et perdait alors toute ancre de titulaire.
    let code_postal_line_regex = Regex::new(r"[[:space:]]*\d{5}\s*[[:alpha:]]").unwrap();
    let code_postal_word_regex = Regex::new(r"^\d{5}").unwrap();

    // la géométrie des lignes lues, pour la trace : une ligne de code postal écartée
    // pour sa hauteur explique un titulaire qu'aucun bloc n'a jamais couvert
    let max_height = max_line_height(&text_lines);
    provenance.ocr_lines = text_lines
        .iter()
        .map(|line| {
            let r = line.bounding_rect();
            let mask = (
                r.left().max(0) as u32,
                r.top().max(0) as u32,
                r.width().max(0) as u32,
                r.height().max(0) as u32,
            );
            OcrLine {
                rect: page_rect(img, mask, turn),
                postal: code_postal_line_regex.is_match(&line.to_string())
                    && line
                        .words()
                        .any(|w| code_postal_word_regex.is_match(&w.to_string())),
                oversized: r.height() > max_height,
            }
        })
        .collect();

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
    let page_lines: Vec<String> = text_lines.iter().map(|l| l.to_string()).collect();

    let scored: Vec<(usize, &Anchor, i32)> = postal_anchors
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
        .collect();
    for (_, anchor, score) in &scored {
        if *score < 0 {
            let skipped = block(anchor.addr_mask(), BlockOutcome::SkippedAsDomiciliation);
            provenance.holder_blocks.push(skipped);
        }
    }
    let mut ranked: Vec<(usize, &Anchor, i32)> = scored
        .into_iter()
        .filter(|(_, _, score)| *score >= 0)
        .collect();
    ranked.sort_by_key(|(_, _, score)| -*score);
    provenance.holder_candidates = ranked.len() as u32;

    // chaque titulaire recevable, avec l'indice de son bloc dans la trace
    let mut account_holders: Vec<(String, usize)> = Vec::new();
    for (index, anchor, score) in ranked {
        provenance.holder_blocks_read += 1;
        let addr_mask = anchor.addr_mask();
        let text = read_mask(index, addr_mask, "addr_mask");
        // un bloc qui se présente comme domiciliation ne devient pas titulaire, même
        // en le recadrant autrement
        if domiciliation.is_match(&text) && !holder_label.is_match(&text) {
            let rejected = block(addr_mask, BlockOutcome::Domiciliation);
            provenance.holder_blocks.push(rejected);
            continue;
        }
        // Un bloc porteur du libellé de titulaire est désigné par le document même sans
        // civilité — le cas de toute personne morale ; `trim_holder` sait s'y ancrer.
        let (text, mask, refusal) = if match_civilite(&text) || holder_label.is_match(&text) {
            (Some(text), addr_mask, BlockOutcome::Trimmed)
        } else {
            let right_mask = anchor.right_align_addr_mask();
            let new_text = read_mask(index, right_mask, "right_align_addr_mask");
            if (match_civilite(&new_text) || holder_label.is_match(&new_text))
                && !domiciliation.is_match(&new_text)
            {
                (Some(new_text), right_mask, BlockOutcome::Trimmed)
            } else if let Some(label) = side_label(&text_lines, anchor, &holder_label) {
                // Libellé à gauche, titulaire aligné à droite sur les mêmes lignes : le
                // libellé sort des deux recadrages, et rien d'autre ne désigne le bloc
                // d'une personne morale sans forme juridique. Il en donne pourtant la
                // première ligne : on recadre de sa hauteur jusqu'au code postal, ce
                // qui laisse dehors la date et le numéro d'agence posés au-dessus, et
                // on lit ce recadrage comme étiqueté.
                let mask = side_mask(anchor, label);
                let side = read_mask(index, mask, "side_label_mask");
                if domiciliation.is_match(&side) {
                    (None, mask, BlockOutcome::Domiciliation)
                } else {
                    (
                        Some(format!("Titulaire\n{}", side)),
                        mask,
                        BlockOutcome::Trimmed,
                    )
                }
            } else {
                (None, addr_mask, BlockOutcome::NotDesignated)
            }
        };
        match text
            .and_then(|t| trim_holder(&t, &code_postal_line_regex))
            .map(|h| complete_cut_words(&h, &page_lines))
        {
            Some(holder) => {
                let labelled = holder_label.is_match(&holder);
                account_holders.push((holder, provenance.holder_blocks.len()));
                provenance
                    .holder_blocks
                    .push(block(mask, BlockOutcome::Outranked));
                // le premier bloc porteur d'un libellé — ou le premier tout court quand la
                // page n'en désigne aucun — suffit : inutile de reconnaître les suivants
                if labelled || score >= 1 {
                    break;
                }
            }
            None => provenance.holder_blocks.push(block(mask, refusal)),
        }
    }
    // à plusieurs candidats, celui qui porte un libellé de titulaire l'emporte
    account_holders.sort_by_key(|(text, _)| !holder_label.is_match(text));

    // `s` porte déjà ses sauts de ligne : les recollecter caractère à caractère
    // aplatirait le titulaire en une seule ligne
    if let Some((holder, kept)) = account_holders.into_iter().next() {
        provenance.holder_blocks[kept].outcome = BlockOutcome::Kept;
        return Some(holder);
    }

    // Le mot « titulaire » se cherche dans les lignes déjà reconnues : relancer la
    // reconnaissance de la page entière pour l'y trouver coûtait un appel complet.
    // « bénéficiaire » n'entre pas dans ce repli : mesuré, le masque à compte de lignes
    // fixe rend des blocs tronqués sur ces mises en page — pire que rien
    let account_holder_word_regex = Regex::new(r"(?i)titulaire").unwrap();
    let account_holder_anchors =
        extract_anchors(text_lines.clone(), &account_holder_word_regex, None);

    for (index, anchor) in account_holder_anchors.iter().enumerate() {
        let mask = anchor.account_holder_mask();
        let cropped_img = crop(
            img,
            mask,
            name,
            &format!(r#"{}_account_holder_mask"#, index),
        );
        let text = image_to_string(cropped_img);
        let holder = account_holder_word_regex
            .is_match(&text)
            .then(|| find_simple_account_holder(&text, 1))
            .flatten();
        let outcome = if holder.is_some() {
            BlockOutcome::Kept
        } else {
            BlockOutcome::NotDesignated
        };
        provenance.holder_blocks.push(block(mask, outcome));
        if holder.is_some() {
            return holder;
        }
    }

    // Dernier recours : ni code postal ni libellé autour du titulaire — le RIB n'imprime
    // que le nom, sans adresse (« M OU MME DUPONT JEAN » sous le BIC). Aucune ancre ne
    // le désignait. La ligne de la page qui commence par une civilité est le titulaire ;
    // le chemin texte a le même recours.
    let line = civility_line(&text_lines)?;
    let r = line.bounding_rect();
    let mask = (
        r.left().max(0) as u32,
        r.top().max(0) as u32,
        r.width().max(0) as u32,
        r.height().max(0) as u32,
    );
    provenance
        .holder_blocks
        .push(block(mask, BlockOutcome::Kept));
    Some(line.to_string().trim().to_string())
}

/// Première ligne de la page, de haut en bas, qui commence par une civilité suivie d'un
/// nom : « M DUPONT », « MME DUPONT », « M OU MME DUPONT », « M.OU MME DUPONT ». Un mot
/// qui commence comme une civilité (« MONTANT », « Mode de paiement ») n'en est pas une.
fn civility_line(text_lines: &[TextLine]) -> Option<&TextLine> {
    let civility = Regex::new(
        r"(?i)^\s*(m|mr|monsieur|mme|madame|mlle|mle|melle|mademoiselle)(\.\s*|\s+)(ou\s+(m|mr|mme|madame|monsieur)\.?\s+)?[[:alpha:]]{2,}",
    )
    .unwrap();

    let mut lines: Vec<&TextLine> = text_lines.iter().collect();
    lines.sort_by_key(|l| l.bounding_rect().top());
    lines
        .into_iter()
        .find(|l| civility.is_match(&l.to_string()))
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

    let kept: Vec<&str> = lines[..end]
        .iter()
        .copied()
        .filter(|line| !is_label_or_noise(line))
        .collect();

    (!kept.is_empty()).then(|| kept.join("\n"))
}

/// Recolle les mots que le bord du recadrage a coupés, d'après la lecture pleine page.
///
/// Le masque d'adresse est calé sur la largeur du code postal : assez pour un nom de
/// personne, pas pour une raison sociale — « ASSOCIATION PALETTE ET PINC »,
/// « ATION DES AMIS DE ». La page, lue une fois pour toutes, porte souvent la ligne
/// entière. On ne complète que le premier et le dernier mot d'une ligne, et seulement
/// quand la ligne se retrouve telle quelle dans la page : jamais de mot ajouté, pour
/// ne pas happer la colonne voisine.
fn complete_cut_words(holder: &str, page_lines: &[String]) -> String {
    holder
        .lines()
        .map(|line| {
            let line = line.trim();
            // une ligne courte se retrouve n'importe où, au milieu d'autres mots : on
            // n'y touche pas
            if line.chars().count() < 8 {
                return line.to_string();
            }
            let is_word = |c: char| c.is_alphanumeric();

            // chaque occurrence, étendue vers la gauche jusqu'au début du mot et vers la
            // droite jusqu'à sa fin — une ligne d'un RIB imprimé en deux ou trois
            // exemplaires se retrouve autant de fois
            let completions: Vec<String> = page_lines
                .iter()
                .flat_map(|page| page.match_indices(line).map(move |(at, _)| (page, at)))
                .map(|(page, at)| {
                    let start = page[..at]
                        .char_indices()
                        .rev()
                        .take_while(|(_, c)| is_word(*c))
                        .last()
                        .map_or(at, |(i, _)| i);
                    let end = at + line.len();
                    let stop = page[end..]
                        .char_indices()
                        .find(|(_, c)| !is_word(*c))
                        .map_or(page.len(), |(i, _)| end + i);
                    page[start..stop].to_string()
                })
                .collect();

            // seulement si toutes les occurrences disent la même chose
            match completions.split_first() {
                Some((first, rest)) if rest.iter().all(|c| c == first) => first.clone(),
                _ => line.to_string(),
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Ligne qui n'appartient pas au titulaire bien qu'elle tombe dans son bloc : un
/// libellé seul sur sa ligne — la traduction « (Account Owner) », « Adresse : » —, la
/// rangée de chiffres du RIB, ou le bruit qu'une photo fait lire dans un filet de
/// tableau (« 2=====k===m=== »).
pub fn is_label_or_noise(line: &str) -> bool {
    let label = Regex::new(
        r"(?i)^\s*\(?\s*(account\s+(holder|owner|name)|adresse|address|rib)\s*\)?\s*:?\s*$",
    )
    .unwrap();
    let digits_only = Regex::new(r"^[\d\s]{12,}$").unwrap();

    let chars: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    let alnum = chars.iter().filter(|c| c.is_alphanumeric()).count();
    let noisy = !chars.is_empty() && alnum * 2 < chars.len();

    label.is_match(line) || digits_only.is_match(line.trim()) || noisy
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

    fn line(text: &str, top: i32, left: i32, height: i32, width: i32) -> TextLine {
        let n = text.chars().count() as i32;
        TextLine::new(
            text.chars()
                .enumerate()
                .map(|(i, c)| crate::lines::TextChar {
                    char: c,
                    rect: crate::lines::Rect::from_tlhw(
                        top,
                        left + i as i32 * width / n,
                        height,
                        width / n,
                    ),
                })
                .collect(),
        )
    }

    /// « Intitulé du compte » à gauche, titulaire aligné à droite : le libellé est
    /// rattaché au bloc du code postal qu'il borde, pas à un code postal plus haut ni à
    /// un libellé posé à droite.
    #[test]
    fn a_label_on_the_left_of_the_block_is_found() {
        let label = Regex::new(r"(?i)(titulaire|intitul[ée])").unwrap();
        // code postal à x = 1000, y = 400, lignes de 20 px
        let postal = Anchor::new(Point::new(1000, 400), Point::new(1100, 420));
        let lines = vec![
            line("21/01/2021", 200, 1400, 20, 200),
            line("Intitulé du compte", 300, 100, 20, 300),
            line("ASSOC. LES AMIS DE CEZANNE", 300, 900, 20, 600),
            line("74103 ANNEMASSE", 400, 1000, 20, 300),
        ];

        // bord droit à un pixel près : l'aide répartit la largeur entre les caractères
        assert_eq!(side_label(&lines, &postal, &label), Some((300, 399)));
        // le recadrage part du libellé, pas de la date au-dessus, et va vers la gauche
        // comme le masque aligné à droite, cinq largeurs de code postal
        let (x, y, _, height) = side_mask(&postal, (300, 400));
        assert_eq!((x, y, y + height), (500, 290, 430));
        // sans passer le libellé quand il est plus près
        assert_eq!(side_mask(&postal, (300, 700)).0, 700);

        // un libellé à droite du code postal ne compte pas
        let right = vec![line("Titulaire", 300, 1500, 20, 200)];
        assert_eq!(side_label(&right, &postal, &label), None);
    }

    /// Une page lue de haut en bas a des lignes plus hautes que larges ; une page droite
    /// non, même avec quelques mots courts ou une étiquette verticale en marge.
    #[test]
    fn a_page_read_top_to_bottom_is_detected() {
        let vertical = vec![
            line("FR76 3000 1000 6449", 0, 0, 400, 30),
            line("M MATISSE HENRI", 0, 40, 300, 30),
            line("44100 NANTES", 0, 80, 250, 30),
        ];
        assert!(mostly_vertical(&vertical));

        let upright = vec![
            line("FR76 3000 1000 6449", 0, 0, 30, 400),
            line("M MATISSE HENRI", 40, 0, 30, 300),
            line("44100 NANTES", 80, 0, 30, 250),
            line("Page 1 sur 1", 0, 900, 200, 20),
        ];
        assert!(!mostly_vertical(&upright));
    }

    /// Un mot coupé par le bord du recadrage est recollé d'après la page ; un mot
    /// entier n'est jamais prolongé, et aucun mot n'est ajouté.
    #[test]
    fn words_cut_by_the_crop_are_completed_from_the_page() {
        let page = vec![
            "ASSOCIATION DES AMIS DE LA PALETTE".to_string(),
            "IBAN FR76 3000 1000".to_string(),
            "ASSOCIATION PALETTE ET PINCEAUX   Domiciliation".to_string(),
            "44100 NANTES".to_string(),
        ];

        assert_eq!(
            complete_cut_words("ATION DES AMIS DE LA PALETTE", &page),
            "ASSOCIATION DES AMIS DE LA PALETTE"
        );
        assert_eq!(
            complete_cut_words("ASSOCIATION PALETTE ET PINC\n44100 NANTES", &page),
            "ASSOCIATION PALETTE ET PINCEAUX\n44100 NANTES"
        );
        // absent de la page : rien ne change
        assert_eq!(
            complete_cut_words("M MATISSE HENRI", &page),
            "M MATISSE HENRI"
        );
        // trop court pour être situé sûrement : « DE » n'est pas prolongé
        assert_eq!(complete_cut_words("DE", &page), "DE");
    }

    /// Libellés seuls sur leur ligne, rangée de chiffres du RIB et bruit de filet ne
    /// font pas partie du titulaire ; ses vraies lignes restent.
    #[test]
    fn labels_and_noise_are_dropped_from_the_block() {
        let text = "Titulaire du compte\n(Account Owner)\nASSOC. LES AMIS DE CEZANNE\nADRESSE :\n12 RUE DES GRIVES\n44100 NANTES";
        assert_eq!(
            trim_holder(text, &postal_code()).as_deref(),
            Some("ASSOC. LES AMIS DE CEZANNE\n12 RUE DES GRIVES\n44100 NANTES")
        );

        assert!(is_label_or_noise("2===============k=========k==m====="));
        assert!(is_label_or_noise("30001 00064 49190095620 88"));
        assert!(is_label_or_noise("RIB"));
        assert!(!is_label_or_noise("BP 10001"));
        assert!(!is_label_or_noise("ASS FAUVE (EX NABIS)"));
        assert!(!is_label_or_noise("ART 'NEUF' DECO 'BIS"));
    }

    /// Un RIB qui n'imprime que le nom, sans adresse : la ligne qui commence par une
    /// civilité est le titulaire, la première de haut en bas.
    #[test]
    fn a_holder_printed_without_address_is_found_by_its_civility() {
        let lines = vec![
            line("Mode de paiement : virement", 100, 100, 20, 500),
            line("FR76 3000 1000 6449 1900 9562 088", 200, 100, 20, 700),
            line("BIC : BDFEFRPPCCT", 240, 100, 20, 300),
            line("M OU MME MONET CLAUDE", 280, 100, 20, 450),
            line("MME KAHLO FRIDA", 400, 100, 20, 300),
        ];
        assert_eq!(
            civility_line(&lines).map(|l| l.to_string()).as_deref(),
            Some("M OU MME MONET CLAUDE")
        );

        let glued = vec![line("M.OU MME MONET CLAUDE", 280, 100, 20, 450)];
        assert!(civility_line(&glued).is_some());

        // un mot qui commence comme une civilité n'en est pas une
        let none = vec![
            line("MONTANT DU VIREMENT", 100, 100, 20, 400),
            line("Mme", 140, 100, 20, 60),
        ];
        assert!(civility_line(&none).is_none());
    }

    /// Une forme juridique en tête de ligne tient lieu de civilité : c'est le début du
    /// titulaire d'une personne morale.
    #[test]
    fn a_legal_form_anchors_a_company_holder() {
        let text =
            "Intitulé du compte\nASSOC. LES AMIS DE CEZANNE\n12 RUE DES GRIVES\n44100 NANTES";
        assert_eq!(
            trim_holder(text, &postal_code()).as_deref(),
            Some("ASSOC. LES AMIS DE CEZANNE\n12 RUE DES GRIVES\n44100 NANTES")
        );
        assert!(match_civilite("SARL LES FAUVES"));
        // en minuscules ou au milieu d'une ligne, ce n'est pas une forme juridique
        assert!(!match_civilite("la classe de 2nde"));
        assert!(!match_civilite("CODE BANQUE SAS 12345"));
    }
}
