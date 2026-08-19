//! Écriture de PDF minimalistes.
//!
//! Écrit à la main plutôt que via une bibliothèque : le corpus doit contenir des
//! formes que les générateurs courants n'exposent pas commodément — page portant deux
//! images distinctes, document multipage dont seule la seconde page porte le RIB,
//! image scannée accompagnée de texte parasite. Un PDF de test n'utilise que les
//! polices de base, donc rien à embarquer.

/// Les 14 polices standard ne nécessitent aucun embarquement.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Font {
    Helvetica,
    HelveticaBold,
    Courier,
    CourierBold,
}

impl Font {
    const ALL: [Font; 4] = [
        Font::Helvetica,
        Font::HelveticaBold,
        Font::Courier,
        Font::CourierBold,
    ];

    fn base_name(&self) -> &'static str {
        match self {
            Font::Helvetica => "Helvetica",
            Font::HelveticaBold => "Helvetica-Bold",
            Font::Courier => "Courier",
            Font::CourierBold => "Courier-Bold",
        }
    }

    fn resource(&self) -> &'static str {
        match self {
            Font::Helvetica => "F1",
            Font::HelveticaBold => "F2",
            Font::Courier => "F3",
            Font::CourierBold => "F4",
        }
    }
}

/// Texte positionné, `y` mesuré depuis le haut de la page.
pub struct Text {
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub font: Font,
    pub content: String,
}

/// Trait de séparation ou bordure de cellule.
pub struct Line {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub width: f32,
}

/// Image JPEG placée sur la page, référencée par son index dans `Pdf::images`.
pub struct PlacedImage {
    pub index: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

pub struct Jpeg {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub grayscale: bool,
}

#[derive(Default)]
pub struct Page {
    pub texts: Vec<Text>,
    pub lines: Vec<Line>,
    pub images: Vec<PlacedImage>,
}

pub const A4_WIDTH: f32 = 595.0;
pub const A4_HEIGHT: f32 = 842.0;

#[derive(Default)]
pub struct Pdf {
    pub pages: Vec<Page>,
    pub images: Vec<Jpeg>,
}

/// Échappement PDF, avec émission des accents en WinAnsi.
///
/// Les translittérer serait plus simple, mais fausserait la mesure : les en-têtes du
/// pipeline sont cherchés sous leur forme accentuée (« Intitulé du compte »), et un
/// corpus sans accents les rendrait introuvables pour de mauvaises raisons.
fn escape(text: &str) -> String {
    let mut out = String::new();

    for c in text.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '(' => out.push_str(r"\("),
            ')' => out.push_str(r"\)"),
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201c}' | '\u{201d}' => out.push('"'),
            c if c.is_ascii() => out.push(c),
            // WinAnsiEncoding recouvre Latin-1 sur cette plage
            c if (c as u32) <= 0xFF => out.push_str(&format!("\\{:03o}", c as u32)),
            _ => out.push('?'),
        }
    }

    out
}

impl Page {
    fn content_stream(&self) -> String {
        let mut out = String::new();

        for image in &self.images {
            // le système de coordonnées PDF part du bas de la page
            let y = A4_HEIGHT - image.y - image.height;
            out.push_str(&format!(
                "q {:.2} 0 0 {:.2} {:.2} {:.2} cm /Im{} Do Q\n",
                image.width, image.height, image.x, y, image.index
            ));
        }

        for line in &self.lines {
            out.push_str(&format!(
                "{:.2} w {:.2} {:.2} m {:.2} {:.2} l S\n",
                line.width,
                line.x1,
                A4_HEIGHT - line.y1,
                line.x2,
                A4_HEIGHT - line.y2
            ));
        }

        for text in &self.texts {
            out.push_str(&format!(
                "BT /{} {:.2} Tf {:.2} {:.2} Td ({}) Tj ET\n",
                text.font.resource(),
                text.size,
                text.x,
                A4_HEIGHT - text.y,
                escape(&text.content)
            ));
        }

        out
    }
}

impl Pdf {
    pub fn render(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();

        out.extend_from_slice(b"%PDF-1.4\n");

        // Numérotation : 1 catalogue, 2 arbre de pages, puis deux objets par page
        // (page et contenu), puis les polices, puis les images.
        let page_count = self.pages.len();
        let first_font = 3 + 2 * page_count;
        let first_image = first_font + Font::ALL.len();

        let push = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
            offsets.push(out.len());
            out.extend_from_slice(body);
        };

        let kids: Vec<String> = (0..page_count)
            .map(|i| format!("{} 0 R", 3 + 2 * i))
            .collect();

        push(
            &mut out,
            &mut offsets,
            b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        );
        push(
            &mut out,
            &mut offsets,
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {} >>\nendobj\n",
                kids.join(" "),
                page_count
            )
            .as_bytes(),
        );

        let fonts: Vec<String> = Font::ALL
            .iter()
            .enumerate()
            .map(|(i, font)| format!("/{} {} 0 R", font.resource(), first_font + i))
            .collect();

        for (i, page) in self.pages.iter().enumerate() {
            let page_id = 3 + 2 * i;
            let content_id = page_id + 1;

            let xobjects: Vec<String> = page
                .images
                .iter()
                .map(|image| format!("/Im{} {} 0 R", image.index, first_image + image.index))
                .collect();

            let resources = if xobjects.is_empty() {
                format!("<< /Font << {} >> >>", fonts.join(" "))
            } else {
                format!(
                    "<< /Font << {} >> /XObject << {} >> >>",
                    fonts.join(" "),
                    xobjects.join(" ")
                )
            };

            push(
                &mut out,
                &mut offsets,
                format!(
                    "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources {} /Contents {} 0 R >>\nendobj\n",
                    page_id, A4_WIDTH, A4_HEIGHT, resources, content_id
                )
                .as_bytes(),
            );

            let stream = page.content_stream();
            push(
                &mut out,
                &mut offsets,
                format!(
                    "{} 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
                    content_id,
                    stream.len(),
                    stream
                )
                .as_bytes(),
            );
        }

        for (i, font) in Font::ALL.iter().enumerate() {
            push(
                &mut out,
                &mut offsets,
                format!(
                    "{} 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding >>\nendobj\n",
                    first_font + i,
                    font.base_name()
                )
                .as_bytes(),
            );
        }

        for (i, image) in self.images.iter().enumerate() {
            let color_space = if image.grayscale {
                "/DeviceGray"
            } else {
                "/DeviceRGB"
            };

            offsets.push(out.len());
            out.extend_from_slice(
                format!(
                    "{} 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace {} /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>\nstream\n",
                    first_image + i,
                    image.width,
                    image.height,
                    color_space,
                    image.bytes.len()
                )
                .as_bytes(),
            );
            out.extend_from_slice(&image.bytes);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        }

        let xref_offset = out.len();
        let object_count = offsets.len() + 1;

        out.extend_from_slice(format!("xref\n0 {}\n", object_count).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", offset).as_bytes());
        }

        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
                object_count, xref_offset
            )
            .as_bytes(),
        );

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_utils::pdf_bytes_to_string;

    fn page_with(content: &str) -> Pdf {
        Pdf {
            pages: vec![Page {
                texts: vec![Text {
                    x: 50.0,
                    y: 100.0,
                    size: 11.0,
                    font: Font::Helvetica,
                    content: content.to_string(),
                }],
                ..Default::default()
            }],
            images: Vec::new(),
        }
    }

    #[test]
    fn produces_a_pdf_readable_by_poppler() {
        let bytes = page_with("FR76 3000 1000 6449 1900 9562 088").render();

        assert!(bytes.starts_with(b"%PDF-"));
        assert!(tree_magic_mini::from_u8(&bytes) == "application/pdf");

        let text = pdf_bytes_to_string(bytes);
        assert!(
            text.contains("FR76 3000 1000 6449 1900 9562 088"),
            "texte extrait : {:?}",
            text
        );
    }

    #[test]
    fn escapes_parentheses_and_backslashes() {
        let text = pdf_bytes_to_string(page_with(r"SCI (A\B) MATISSE").render());

        assert!(
            text.contains(r"SCI (A\B) MATISSE"),
            "texte extrait : {:?}",
            text
        );
    }

    /// Les accents doivent survivre au aller-retour : les en-têtes que cherche le
    /// pipeline en portent.
    #[test]
    fn keeps_accents_through_winansi() {
        let text = pdf_bytes_to_string(page_with("Intitulé du compte / Vendée").render());

        assert!(
            text.contains("Intitulé du compte / Vendée"),
            "texte extrait : {:?}",
            text
        );
    }

    #[test]
    fn multipage_keeps_pages_separate() {
        let pdf = Pdf {
            pages: vec![
                Page {
                    texts: vec![Text {
                        x: 50.0,
                        y: 100.0,
                        size: 11.0,
                        font: Font::Helvetica,
                        content: "PREMIERE PAGE".to_string(),
                    }],
                    ..Default::default()
                },
                Page {
                    texts: vec![Text {
                        x: 50.0,
                        y: 100.0,
                        size: 11.0,
                        font: Font::Helvetica,
                        content: "SECONDE PAGE".to_string(),
                    }],
                    ..Default::default()
                },
            ],
            images: Vec::new(),
        };

        let text = pdf_bytes_to_string(pdf.render());

        assert!(text.contains("PREMIERE PAGE"));
        assert!(text.contains("SECONDE PAGE"));
    }
}
