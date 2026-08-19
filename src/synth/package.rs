//! Mise en forme finale d'un RIB synthétique.
//!
//! C'est le conditionnement, et non le contenu, qui décide de la branche empruntée dans
//! `analysis::vec_to_rib`. Un même RIB est donc décliné en PDF natif, en PDF scanné et
//! en photo, y compris dans des variantes dont on sait qu'elles échouent aujourd'hui —
//! mieux vaut un échec mesuré qu'un angle mort.

use image::DynamicImage;

use super::data::RibData;
use super::degrade::{self, Degradation};
use super::layout::{self, Layout};
use super::pdf::{Font, Jpeg, Page, Pdf, PlacedImage, Text, A4_HEIGHT, A4_WIDTH};
use super::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Form {
    /// PDF vectoriel : `pdftotext` suffit, aucune OCR.
    PdfText,
    /// Scan nu : `pdftotext` ne renvoie rien, le pipeline rasterise.
    PdfImg,
    /// Scan accompagné de texte parasite sans IBAN : `pdftotext` renvoie du texte
    /// inexploitable, et c'est le garde-fou `list_img_in_pdf == 1` qui décide.
    PdfImgText,
    /// Logo vectorisé en plus du scan, soit deux images : `list_img_in_pdf != 1`.
    PdfImgMulti,
    /// Courrier d'accompagnement en page 1, RIB en page 2 : `pdftoppm -singlefile`
    /// ne rasterise que la première page.
    PdfPage2,
    Jpeg,
    /// JPEG dont l'image est stockée pivotée, redressée par le tag EXIF Orientation.
    JpegExif,
    Png,
}

impl Form {
    /// Étiquette reportée dans la colonne `src` du rapport.
    pub fn tag(&self) -> &'static str {
        match self {
            Form::PdfText => "pdf_text",
            Form::PdfImg => "pdf_img",
            Form::PdfImgText => "pdf_img_text",
            Form::PdfImgMulti => "pdf_img_multi",
            Form::PdfPage2 => "pdf_p2",
            Form::Jpeg => "jpeg",
            Form::JpegExif => "jpeg_exif",
            Form::Png => "png",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            Form::Jpeg | Form::JpegExif => "jpg",
            Form::Png => "png",
            _ => "pdf",
        }
    }

    /// Formes que le pipeline ne sait pas traiter en l'état. Elles restent dans le
    /// corpus, mais hors du taux de réussite : ce sont des cibles, pas des régressions.
    pub fn known_failure(&self) -> bool {
        matches!(self, Form::PdfImgMulti | Form::PdfPage2)
    }
}

pub struct Sample {
    pub name: String,
    pub bytes: Vec<u8>,
    pub form: Form,
    pub recipe: String,
    pub data: RibData,
}

/// Segment APP1 portant le seul tag Orientation.
fn exif_orientation_segment(orientation: u16) -> Vec<u8> {
    let mut payload = Vec::new();

    payload.extend_from_slice(b"Exif\0\0");
    payload.extend_from_slice(b"II\x2a\x00"); // petit-boutiste
    payload.extend_from_slice(&8u32.to_le_bytes()); // décalage du premier IFD
    payload.extend_from_slice(&1u16.to_le_bytes()); // une seule entrée
    payload.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
    payload.extend_from_slice(&3u16.to_le_bytes()); // SHORT
    payload.extend_from_slice(&1u32.to_le_bytes()); // un élément
    payload.extend_from_slice(&orientation.to_le_bytes());
    payload.extend_from_slice(&0u16.to_le_bytes()); // bourrage de la valeur sur 4 octets
    payload.extend_from_slice(&0u32.to_le_bytes()); // pas d'IFD suivant

    let mut segment = vec![0xFF, 0xE1];
    segment.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(&payload);

    segment
}

/// Insère le segment EXIF juste après le marqueur de début d'image.
fn with_exif_orientation(jpeg: Vec<u8>, orientation: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    out.extend_from_slice(&exif_orientation_segment(orientation));
    out.extend_from_slice(&jpeg[2..]);

    out
}

/// Enveloppe une image dans un PDF, en la cadrant sur la page.
fn image_page(img: &DynamicImage, quality: u8) -> (Jpeg, PlacedImage) {
    let bytes = degrade::to_jpeg(img, quality);
    let (width, height) = (img.width(), img.height());

    let scale = (A4_WIDTH / width as f32).min(A4_HEIGHT / height as f32);
    let (drawn_width, drawn_height) = (width as f32 * scale, height as f32 * scale);

    (
        Jpeg {
            bytes,
            width,
            height,
            grayscale: true,
        },
        PlacedImage {
            index: 0,
            x: (A4_WIDTH - drawn_width) / 2.0,
            y: (A4_HEIGHT - drawn_height) / 2.0,
            width: drawn_width,
            height: drawn_height,
        },
    )
}

fn note(page: &mut Page, x: f32, y: f32, size: f32, content: &str) {
    page.texts.push(Text {
        x,
        y,
        size,
        font: Font::Helvetica,
        content: content.to_string(),
    });
}

/// Petit aplat servant de logo : sa seule raison d'être est de porter le nombre
/// d'images de la page à deux.
fn logo() -> Jpeg {
    let img = DynamicImage::ImageLuma8(image::ImageBuffer::from_fn(64, 64, |x, y| {
        image::Luma([(((x / 8) + (y / 8)) % 2 * 120 + 60) as u8])
    }));

    Jpeg {
        bytes: degrade::to_jpeg(&img, 80),
        width: 64,
        height: 64,
        grayscale: true,
    }
}

fn package(form: Form, img: &DynamicImage, quality: u8) -> Vec<u8> {
    let (jpeg, placed) = image_page(img, quality);

    match form {
        Form::Jpeg => degrade::to_jpeg(img, quality),
        Form::JpegExif => with_exif_orientation(degrade::to_jpeg(&img.rotate270(), quality), 6),
        Form::Png => degrade::to_png(img),

        Form::PdfImg => Pdf {
            pages: vec![Page {
                images: vec![placed],
                ..Default::default()
            }],
            images: vec![jpeg],
        }
        .render(),

        Form::PdfImgText => {
            let mut page = Page {
                images: vec![placed],
                ..Default::default()
            };
            note(
                &mut page,
                40.0,
                30.0,
                9.0,
                "Votre releve d'identite bancaire",
            );
            note(&mut page, 40.0, A4_HEIGHT - 20.0, 7.0, "Page 1 / 1");

            Pdf {
                pages: vec![page],
                images: vec![jpeg],
            }
            .render()
        }

        Form::PdfImgMulti => {
            let mut page = Page {
                images: vec![
                    placed,
                    PlacedImage {
                        index: 1,
                        x: 40.0,
                        y: 20.0,
                        width: 48.0,
                        height: 48.0,
                    },
                ],
                ..Default::default()
            };
            note(&mut page, 100.0, 40.0, 9.0, "Votre banque en ligne");

            Pdf {
                pages: vec![page],
                images: vec![jpeg, logo()],
            }
            .render()
        }

        Form::PdfPage2 => {
            let mut cover = Page::default();
            note(&mut cover, 60.0, 80.0, 11.0, "Madame, Monsieur,");
            note(
                &mut cover,
                60.0,
                110.0,
                10.0,
                "Vous trouverez en page suivante le releve d'identite bancaire",
            );
            note(&mut cover, 60.0, 126.0, 10.0, "que vous nous avez demande.");

            Pdf {
                pages: vec![
                    cover,
                    Page {
                        images: vec![placed],
                        ..Default::default()
                    },
                ],
                images: vec![jpeg],
            }
            .render()
        }

        Form::PdfText => unreachable!("le PDF natif ne passe pas par une image"),
    }
}

/// Décline un RIB en un échantillon d'une forme et d'une recette données.
pub fn build(
    index: usize,
    layout: &Layout,
    data: RibData,
    form: Form,
    degradation: &Degradation,
    rng: &mut Rng,
) -> Sample {
    // Le gabarit sans adresse ne pose que le nom : la vérité terrain doit suivre.
    let data = if layout.name == "cepac" {
        RibData {
            holder_lines: data
                .holder_lines
                .iter()
                .take_while(|line| !line.chars().next().is_some_and(|c| c.is_ascii_digit()))
                .cloned()
                .collect(),
            ..data
        }
    } else {
        data
    };

    let pdf = layout::render(layout, &data).render();

    let (bytes, recipe) = if form == Form::PdfText {
        (pdf, "natif".to_string())
    } else {
        let raster =
            degrade::rasterize(&pdf, degrade::dpi_for_cap_height(degradation.cap_height), 1);
        let degraded = degrade::apply(&raster, degradation, rng);
        let quality = degradation.jpeg_quality.unwrap_or(92);

        (package(form, &degraded, quality), degradation.recipe())
    };

    Sample {
        name: format!(
            "{:03}_{}_{}_{}.{}",
            index,
            layout.name,
            form.tag(),
            recipe,
            form.extension()
        ),
        bytes,
        form,
        recipe,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_utils::{list_img_in_pdf, pdf_bytes_to_string};

    fn sample(form: Form, cap_height: u32) -> Sample {
        let mut rng = Rng::new(11);
        let data = crate::synth::data::generate(&mut rng);

        build(
            0,
            &layout::LAYOUTS[0],
            data,
            form,
            &Degradation {
                cap_height,
                ..Default::default()
            },
            &mut rng,
        )
    }

    #[test]
    fn native_pdf_carries_extractable_text() {
        let sample = sample(Form::PdfText, 30);

        assert_eq!(tree_magic_mini::from_u8(&sample.bytes), "application/pdf");
        assert!(pdf_bytes_to_string(sample.bytes).contains("FR"));
    }

    #[test]
    fn scanned_pdf_carries_no_text_but_one_image() {
        let sample = sample(Form::PdfImg, 20);

        assert_eq!(tree_magic_mini::from_u8(&sample.bytes), "application/pdf");
        assert!(pdf_bytes_to_string(sample.bytes.clone()).trim().is_empty());
        assert_eq!(list_img_in_pdf(sample.bytes), 1);
    }

    /// La variante qui exerce le garde-fou : du texte, mais pas d'IBAN dedans.
    #[test]
    fn scanned_pdf_with_junk_text_still_holds_a_single_image() {
        let sample = sample(Form::PdfImgText, 20);
        let text = pdf_bytes_to_string(sample.bytes.clone());

        assert!(!text.trim().is_empty());
        assert!(!text.contains("FR76"));
        assert_eq!(list_img_in_pdf(sample.bytes), 1);
    }

    /// Échec connu : deux images sur la page désarment le garde-fou.
    #[test]
    fn multi_image_pdf_defeats_the_single_image_guard() {
        let sample = sample(Form::PdfImgMulti, 20);

        assert!(Form::PdfImgMulti.known_failure());
        assert_eq!(list_img_in_pdf(sample.bytes), 2);
    }

    #[test]
    fn second_page_pdf_has_two_pages() {
        let sample = sample(Form::PdfPage2, 20);

        assert!(Form::PdfPage2.known_failure());
        assert!(pdf_bytes_to_string(sample.bytes).contains("page suivante"));
    }

    #[test]
    fn photos_are_plain_images() {
        assert_eq!(
            tree_magic_mini::from_u8(&sample(Form::Jpeg, 20).bytes),
            "image/jpeg"
        );
        assert_eq!(
            tree_magic_mini::from_u8(&sample(Form::Png, 20).bytes),
            "image/png"
        );
    }

    /// L'image est stockée pivotée et redressée par le tag : le pipeline doit honorer
    /// l'orientation EXIF pour retomber sur ses pieds.
    #[test]
    fn exif_variant_is_stored_rotated() {
        let sample = sample(Form::JpegExif, 20);
        let plain = self::sample(Form::Jpeg, 20);

        assert_eq!(tree_magic_mini::from_u8(&sample.bytes), "image/jpeg");

        let rotated = image::load_from_memory(&sample.bytes).expect("JPEG lisible");
        let upright = image::load_from_memory(&plain.bytes).expect("JPEG lisible");

        // sans appliquer l'EXIF, la variante est en paysage là où l'original est en portrait
        assert_eq!(rotated.width(), upright.height());
        assert_eq!(rotated.height(), upright.width());
    }
}
