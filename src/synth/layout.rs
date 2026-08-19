//! Gabarits de RIB, calqués sur les mises en page réellement observées dans
//! `tests/fixtures/rib/*.txt`.
//!
//! Chacun place le tableau « Code banque / Guichet / N° de compte / Clé RIB », la ligne
//! IBAN, le BIC, le bloc titulaire et un bloc de domiciliation. Ce dernier est un leurre
//! délibéré : il porte lui aussi une adresse avec code postal, et le pipeline ne doit
//! pas le prendre pour le titulaire.

use super::data::RibData;
use super::pdf::{Font, Line, Page, Pdf, Text, A4_WIDTH};

pub struct Layout {
    pub name: &'static str,
    build: fn(&mut Page, &RibData),
}

pub const LAYOUTS: [Layout; 7] = [
    Layout {
        name: "bp",
        build: banque_populaire,
    },
    Layout {
        name: "sg",
        build: societe_generale,
    },
    Layout {
        name: "lcl",
        build: lcl,
    },
    Layout {
        name: "postale",
        build: banque_postale,
    },
    Layout {
        name: "entreprise",
        build: entreprise,
    },
    Layout {
        name: "cic",
        build: cic,
    },
    Layout {
        name: "cepac",
        build: caisse_epargne_pac,
    },
];

/// `FR7630001…` → `FR76 3000 1000 6449 1900 9562 088`
pub fn grouped_iban(iban: &str) -> String {
    iban.chars()
        .collect::<Vec<char>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<String>>()
        .join(" ")
}

/// Largeur approchée d'une chaîne : Courier est à chasse fixe, Helvetica est estimée.
/// Suffisant pour aligner à droite sur un document de test.
fn text_width(content: &str, size: f32, font: Font) -> f32 {
    let ratio = match font {
        Font::Courier | Font::CourierBold => 0.60,
        Font::Helvetica => 0.50,
        Font::HelveticaBold => 0.55,
    };

    content.chars().count() as f32 * size * ratio
}

fn text(page: &mut Page, x: f32, y: f32, size: f32, font: Font, content: &str) {
    page.texts.push(Text {
        x,
        y,
        size,
        font,
        content: content.to_string(),
    });
}

fn right_text(page: &mut Page, right: f32, y: f32, size: f32, font: Font, content: &str) {
    text(
        page,
        right - text_width(content, size, font),
        y,
        size,
        font,
        content,
    );
}

/// Empile des lignes et renvoie l'ordonnée juste sous le bloc.
fn block(page: &mut Page, x: f32, y: f32, size: f32, font: Font, lines: &[String]) -> f32 {
    let leading = size * 1.45;

    for (i, line) in lines.iter().enumerate() {
        text(page, x, y + i as f32 * leading, size, font, line);
    }

    y + lines.len() as f32 * leading
}

fn rule(page: &mut Page, x1: f32, y: f32, x2: f32) {
    page.lines.push(Line {
        x1,
        y1: y,
        x2,
        y2: y,
        width: 0.6,
    });
}

/// Tableau RIB encadré : quatre colonnes libellées, valeurs dessous.
fn rib_table(page: &mut Page, y: f32, x: f32, headers: [&str; 4], data: &RibData, boxed: bool) {
    let columns = [x, x + 105.0, x + 195.0, x + 305.0];
    let values = [
        data.bank.code.clone(),
        data.branch.clone(),
        data.account.clone(),
        data.rib_key.clone(),
    ];

    for (i, header) in headers.iter().enumerate() {
        text(page, columns[i], y, 8.0, Font::HelveticaBold, header);
        text(page, columns[i], y + 18.0, 10.0, Font::Courier, &values[i]);
    }

    if boxed {
        // les traits de cellule qui frôlent les chiffres sont une source connue
        // d'erreurs d'OCR : le corpus doit en contenir
        rule(page, x - 6.0, y - 10.0, x + 380.0);
        rule(page, x - 6.0, y + 5.0, x + 380.0);
        rule(page, x - 6.0, y + 24.0, x + 380.0);

        for column in columns.iter().chain([x + 380.0].iter()) {
            page.lines.push(Line {
                x1: column - 6.0,
                y1: y - 10.0,
                x2: column - 6.0,
                y2: y + 24.0,
                width: 0.6,
            });
        }
    }
}

fn legal_blurb(page: &mut Page, x: f32, y: f32) {
    let lines = [
        "Ce relevé est destiné à être remis, sur leur demande,",
        "à vos créanciers ou débiteurs appelés à faire inscrire",
        "des opérations à votre compte (virements, paiements de",
        "quittances, etc.). Son utilisation vous garantit le bon",
        "enregistrement des opérations en cause.",
    ];

    for (i, line) in lines.iter().enumerate() {
        text(page, x, y + i as f32 * 10.0, 6.5, Font::Helvetica, line);
    }
}

fn banque_populaire(page: &mut Page, data: &RibData) {
    text(
        page,
        340.0,
        60.0,
        8.0,
        Font::HelveticaBold,
        "Relevé d'Identité Bancaire / Bank details",
    );
    legal_blurb(page, 340.0, 78.0);

    text(
        page,
        60.0,
        95.0,
        9.0,
        Font::HelveticaBold,
        "Titulaire du compte / Account holder",
    );
    block(page, 60.0, 120.0, 10.0, Font::Helvetica, &data.holder_lines);

    text(page, 60.0, 210.0, 8.0, Font::HelveticaBold, "IBAN");
    text(page, 330.0, 210.0, 8.0, Font::HelveticaBold, "BIC");
    text(
        page,
        60.0,
        228.0,
        11.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );
    text(page, 330.0, 228.0, 11.0, Font::Courier, &data.bank.bic);

    rib_table(
        page,
        275.0,
        60.0,
        ["Code Banque", "Code guichet", "N° du compte", "Clé RIB"],
        data,
        false,
    );

    text(
        page,
        60.0,
        340.0,
        8.0,
        Font::HelveticaBold,
        "Domiciliation / Paying Bank",
    );
    block(page, 60.0, 358.0, 9.0, Font::Helvetica, &data.branch_lines);
}

/// Titulaire à gauche, agence alignée à droite : c'est cette mise en page qui met en
/// difficulté un masque de recadrage calé sur un alignement à gauche.
fn societe_generale(page: &mut Page, data: &RibData) {
    text(
        page,
        170.0,
        60.0,
        12.0,
        Font::HelveticaBold,
        "RELEVÉ D'IDENTITÉ BANCAIRE / IBAN",
    );

    text(page, 60.0, 100.0, 9.0, Font::HelveticaBold, "Titulaire");
    block(page, 60.0, 118.0, 10.0, Font::Helvetica, &data.holder_lines);

    right_text(
        page,
        A4_WIDTH - 60.0,
        100.0,
        9.0,
        Font::HelveticaBold,
        "Agence de domiciliation",
    );
    for (i, line) in data.branch_lines.iter().enumerate() {
        right_text(
            page,
            A4_WIDTH - 60.0,
            118.0 + i as f32 * 14.0,
            9.0,
            Font::Helvetica,
            line,
        );
    }

    text(page, 60.0, 215.0, 9.0, Font::HelveticaBold, "RIB");
    rib_table(
        page,
        200.0,
        150.0,
        ["Banque", "Guichet", "Compte", "Clé RIB"],
        data,
        false,
    );

    text(page, 60.0, 275.0, 9.0, Font::HelveticaBold, "IBAN");
    text(
        page,
        150.0,
        275.0,
        11.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );

    text(page, 60.0, 300.0, 9.0, Font::HelveticaBold, "BIC");
    text(page, 150.0, 300.0, 11.0, Font::Courier, &data.bank.bic);
}

/// Libellé du titulaire sur deux lignes et valeur en retrait, tableau encadré.
fn lcl(page: &mut Page, data: &RibData) {
    text(page, 60.0, 70.0, 9.0, Font::HelveticaBold, "TITULAIRE DU");
    text(page, 60.0, 84.0, 9.0, Font::HelveticaBold, "COMPTE :");
    block(page, 150.0, 84.0, 10.0, Font::Helvetica, &data.holder_lines);

    text(page, 60.0, 190.0, 9.0, Font::HelveticaBold, "IBAN :");
    text(
        page,
        130.0,
        190.0,
        11.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );

    text(page, 60.0, 215.0, 9.0, Font::HelveticaBold, "BIC :");
    text(page, 130.0, 215.0, 11.0, Font::Courier, &data.bank.bic);

    rib_table(
        page,
        280.0,
        60.0,
        ["BANQUE", "INDICATIF", "NUMERO DE COMPTE", "CLEF"],
        data,
        true,
    );

    text(
        page,
        60.0,
        360.0,
        9.0,
        Font::HelveticaBold,
        "DOMICILIATION :",
    );
    block(page, 170.0, 360.0, 9.0, Font::Helvetica, &data.branch_lines);
}

/// Aucun libellé « titulaire » : le bloc d'adresse est seul, ce qui force la détection
/// à s'appuyer sur la civilité et le code postal.
fn banque_postale(page: &mut Page, data: &RibData) {
    text(
        page,
        60.0,
        60.0,
        11.0,
        Font::HelveticaBold,
        "RELEVÉ D'IDENTITÉ BANCAIRE",
    );

    block(
        page,
        330.0,
        110.0,
        10.0,
        Font::Helvetica,
        &data.holder_lines,
    );

    rib_table(
        page,
        220.0,
        60.0,
        ["Établissement", "Guichet", "N° de compte", "Clé"],
        data,
        true,
    );

    text(page, 60.0, 300.0, 8.0, Font::HelveticaBold, "IBAN");
    text(
        page,
        60.0,
        318.0,
        11.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );

    text(page, 330.0, 300.0, 8.0, Font::HelveticaBold, "BIC");
    text(page, 330.0, 318.0, 11.0, Font::Courier, &data.bank.bic);

    text(page, 60.0, 370.0, 8.0, Font::HelveticaBold, "DOMICILIATION");
    block(page, 60.0, 388.0, 9.0, Font::Helvetica, &data.branch_lines);
}

/// Mise en page compacte, titulaire personne morale sans civilité.
fn entreprise(page: &mut Page, data: &RibData) {
    text(
        page,
        60.0,
        60.0,
        10.0,
        Font::HelveticaBold,
        "RELEVÉ D'IDENTITÉ BANCAIRE",
    );

    text(
        page,
        60.0,
        95.0,
        8.0,
        Font::HelveticaBold,
        "Intitulé du compte",
    );
    block(page, 60.0, 112.0, 10.0, Font::Helvetica, &data.holder_lines);

    text(page, 60.0, 185.0, 8.0, Font::HelveticaBold, "IBAN");
    text(
        page,
        130.0,
        185.0,
        10.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );

    text(page, 60.0, 205.0, 8.0, Font::HelveticaBold, "BIC");
    text(page, 130.0, 205.0, 10.0, Font::Courier, &data.bank.bic);

    rib_table(
        page,
        245.0,
        60.0,
        ["Code banque", "Code guichet", "Numéro de compte", "Clé RIB"],
        data,
        true,
    );

    text(page, 60.0, 320.0, 8.0, Font::HelveticaBold, "Domiciliation");
    block(page, 60.0, 337.0, 9.0, Font::Helvetica, &data.branch_lines);
}

/// Domiciliation à gauche avec son propre code postal, titulaire à droite en regard.
/// Structure observée sur un document réel où le titulaire était tronqué : deux codes
/// postaux sur la même ligne, et seul le premier — celui de l'agence — était examiné.
fn cic(page: &mut Page, data: &RibData) {
    text(page, 250.0, 60.0, 12.0, Font::HelveticaBold, "RIB");

    rib_table(
        page,
        95.0,
        60.0,
        ["Banque", "Guichet", "N° compte", "Clé"],
        data,
        false,
    );
    text(page, 460.0, 95.0, 8.0, Font::HelveticaBold, "Domiciliation");
    text(
        page,
        460.0,
        113.0,
        9.0,
        Font::Helvetica,
        &data.branch_lines[0],
    );

    text(page, 60.0, 165.0, 9.0, Font::HelveticaBold, "IBAN");
    text(
        page,
        130.0,
        165.0,
        11.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );
    text(page, 400.0, 165.0, 9.0, Font::HelveticaBold, "BIC");
    text(page, 440.0, 165.0, 11.0, Font::Courier, &data.bank.bic);

    // les deux blocs adressés, côte à côte, sur les mêmes lignes
    text(page, 60.0, 220.0, 9.0, Font::HelveticaBold, "Domiciliation");
    block(page, 60.0, 238.0, 10.0, Font::Helvetica, &data.branch_lines);
    text(
        page,
        60.0,
        238.0 + 3.0 * 14.5,
        8.0,
        Font::Helvetica,
        "tél 02 40 00 00 00",
    );

    text(
        page,
        320.0,
        220.0,
        9.0,
        Font::HelveticaBold,
        "Titulaire du compte (Account Owner)",
    );
    block(
        page,
        320.0,
        238.0,
        10.0,
        Font::Helvetica,
        &data.holder_lines,
    );
}

/// Libellé sur deux lignes, « TITULAIRE DU » puis « COMPTE : », le titulaire commençant
/// à droite du libellé dès la première ligne, sans adresse. Structure observée sur un
/// document réel où la première ligne du couple était perdue.
fn caisse_epargne_pac(page: &mut Page, data: &RibData) {
    text(
        page,
        60.0,
        60.0,
        11.0,
        Font::HelveticaBold,
        "RELEVÉ D'IDENTITÉ BANCAIRE",
    );

    text(page, 60.0, 110.0, 9.0, Font::HelveticaBold, "TITULAIRE DU");
    text(page, 60.0, 124.0, 9.0, Font::HelveticaBold, "COMPTE :");

    // le titulaire seul, sans adresse, en regard des deux lignes du libellé
    let name_lines: Vec<String> = data
        .holder_lines
        .iter()
        .take_while(|line| !line.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .cloned()
        .collect();
    block(page, 150.0, 110.0, 10.0, Font::Helvetica, &name_lines);

    text(page, 60.0, 190.0, 9.0, Font::HelveticaBold, "IBAN");
    text(
        page,
        130.0,
        190.0,
        11.0,
        Font::Courier,
        &grouped_iban(&data.iban),
    );
    text(page, 60.0, 212.0, 9.0, Font::HelveticaBold, "BIC");
    text(page, 130.0, 212.0, 11.0, Font::Courier, &data.bank.bic);

    rib_table(
        page,
        260.0,
        60.0,
        ["Code banque", "Code guichet", "N° de compte", "Clé RIB"],
        data,
        true,
    );

    text(page, 60.0, 330.0, 8.0, Font::HelveticaBold, "Domiciliation");
    block(page, 60.0, 348.0, 9.0, Font::Helvetica, &data.branch_lines);
}

/// Rend un gabarit en PDF d'une page.
pub fn render(layout: &Layout, data: &RibData) -> Pdf {
    let mut page = Page::default();
    (layout.build)(&mut page, data);

    // en tête plutôt qu'en pied : une mention en bas de page A4 étendrait la boîte
    // englobante du contenu et neutraliserait le recadrage des variantes photo
    text(
        &mut page,
        60.0,
        32.0,
        7.0,
        Font::Helvetica,
        "DOCUMENT DE TEST GÉNÉRÉ - DONNÉES FICTIVES",
    );

    Pdf {
        pages: vec![page],
        images: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_utils::pdf_bytes_to_string;
    use crate::rib::Rib;
    use crate::synth::data;
    use crate::synth::rng::Rng;

    #[test]
    fn grouping_matches_the_usual_presentation() {
        assert_eq!(
            grouped_iban("FR7630001000644919009562088"),
            "FR76 3000 1000 6449 1900 9562 088"
        );
    }

    /// Un RIB synthétique non dégradé doit être lu sans effort par le chemin
    /// `pdftotext`. Si ce test échoue, c'est le gabarit qui est irréaliste, pas le
    /// pipeline qui est mauvais.
    #[test]
    fn every_layout_is_parsed_from_native_pdf() {
        let mut rng = Rng::new(4);

        for layout in LAYOUTS.iter() {
            for _ in 0..6 {
                let data = data::generate(&mut rng);
                let text = pdf_bytes_to_string(render(layout, &data).render());
                let parsed = Rib::parse(text);

                let parsed = parsed
                    .unwrap_or_else(|| panic!("aucun RIB extrait du gabarit {}", layout.name));

                assert_eq!(
                    parsed.iban(),
                    grouped_iban(&data.iban),
                    "IBAN erroné sur le gabarit {}",
                    layout.name
                );
            }
        }
    }
}
