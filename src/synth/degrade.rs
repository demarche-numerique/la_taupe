//! Dégradations contrôlées d'un RIB rasterisé.
//!
//! Chaque paramètre est explicite et figure dans le nom du fichier produit, si bien que
//! le banc ne rend pas un taux global mais des courbes : à quelle hauteur de capitale,
//! à quel angle, à quel niveau de bruit la reconnaissance cède.

use std::io::{Cursor, Write};
use std::process::{Command, Stdio};

use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageBuffer, Luma, Rgb};
use imageproc::geometric_transformations::{warp_into, Interpolation, Projection};

use crate::image_utils::rotate;

use super::rng::Rng;

/// Hauteur de capitale, en points typographiques, de la ligne d'IBAN des gabarits.
/// Sert à convertir une hauteur cible en pixels vers une résolution de rastérisation.
const IBAN_CAP_HEIGHT_PT: f32 = 11.0 * 0.72;

/// Résolution donnant la hauteur de capitale demandée sur la ligne d'IBAN.
///
/// À titre de repère, la valeur par défaut de `pdftoppm` (150 dpi) ne produit qu'une
/// vingtaine de pixels de haut.
pub fn dpi_for_cap_height(cap_height_px: u32) -> u32 {
    (cap_height_px as f32 * 72.0 / IBAN_CAP_HEIGHT_PT).round() as u32
}

/// Rasterise la page demandée d'un PDF à une résolution donnée.
pub fn rasterize(pdf: &[u8], dpi: u32, page: u32) -> DynamicImage {
    let mut child = Command::new("pdftoppm")
        .args([
            "-png",
            "-r",
            &dpi.to_string(),
            "-f",
            &page.to_string(),
            "-l",
            &page.to_string(),
            "-singlefile",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("pdftoppm est requis pour générer le corpus");

    let mut stdin = child.stdin.take().expect("stdin de pdftoppm");
    let owned = pdf.to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&owned);
    });

    let output = child.wait_with_output().expect("pdftoppm");

    image::load_from_memory(&output.stdout).expect("PNG produit par pdftoppm")
}

/// Recette de dégradation. Le `Default` est le document propre, tel que scanné à plat.
#[derive(Clone, Debug, PartialEq)]
pub struct Degradation {
    /// Hauteur de capitale visée sur la ligne d'IBAN, en pixels.
    pub cap_height: u32,
    /// Quarts de tour horaires, comme une photo prise appareil tourné. Distinct de
    /// `rotation_deg` : sans perte, et non détectable par la mesure d'inclinaison.
    pub quarter_turns: u8,
    pub rotation_deg: f32,
    /// Amplitude du basculement, en fraction de la largeur. 0 = document à plat.
    pub perspective: f32,
    pub blur_sigma: f32,
    /// Écart-type du bruit, en niveaux sur 255.
    pub noise: f32,
    /// Amplitude de l'éclairage inégal, de 0 (uniforme) à 1.
    pub illumination: f32,
    /// Document posé sur un support visible plutôt que détouré.
    pub background: bool,
    pub jpeg_quality: Option<u8>,
}

impl Default for Degradation {
    fn default() -> Self {
        Degradation {
            cap_height: 30,
            quarter_turns: 0,
            rotation_deg: 0.0,
            perspective: 0.0,
            blur_sigma: 0.0,
            noise: 0.0,
            illumination: 0.0,
            background: false,
            jpeg_quality: None,
        }
    }
}

impl Degradation {
    /// Nom de recette, repris dans le nom de fichier : c'est ce qui permet de croiser
    /// un échec avec le paramètre qui l'a provoqué.
    pub fn recipe(&self) -> String {
        let mut parts = vec![format!("h{}", self.cap_height)];

        if !self.quarter_turns.is_multiple_of(4) {
            parts.push(format!("turn{}", 90 * (self.quarter_turns % 4) as u16));
        }
        if self.rotation_deg != 0.0 {
            parts.push(format!("rot{}", (self.rotation_deg * 10.0).round() as i32));
        }
        if self.perspective > 0.0 {
            parts.push(format!(
                "persp{}",
                (self.perspective * 100.0).round() as i32
            ));
        }
        if self.blur_sigma > 0.0 {
            parts.push(format!("blur{}", (self.blur_sigma * 10.0).round() as i32));
        }
        if self.noise > 0.0 {
            parts.push(format!("noise{}", self.noise.round() as i32));
        }
        if self.illumination > 0.0 {
            parts.push(format!(
                "illum{}",
                (self.illumination * 100.0).round() as i32
            ));
        }
        if self.background {
            parts.push("bg".to_string());
        }
        if let Some(quality) = self.jpeg_quality {
            parts.push(format!("q{}", quality));
        }

        parts.join("_")
    }

    /// Vrai pour les recettes qui imitent une photo prise au téléphone.
    pub fn is_photo(&self) -> bool {
        self.background || self.perspective > 0.0
    }
}

/// Recadre sur la zone imprimée, avec une marge. Une photo cadre le document, là où un
/// scan conserve la page entière et ses blancs.
fn crop_to_content(img: &DynamicImage, margin_ratio: f32) -> DynamicImage {
    let gray = img.to_luma8();
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0u32, 0u32);

    for (x, y, pixel) in gray.enumerate_pixels() {
        if pixel.0[0] < 240 {
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }

    if min_x > max_x || min_y > max_y {
        return img.clone();
    }

    let margin = ((max_x - min_x) as f32 * margin_ratio) as u32;

    let x = min_x.saturating_sub(margin);
    let y = min_y.saturating_sub(margin);
    let width = (max_x + margin).min(img.width() - 1) - x + 1;
    let height = (max_y + margin).min(img.height() - 1) - y + 1;

    img.crop_imm(x, y, width, height)
}

/// Bascule le document comme s'il était photographié de biais.
fn apply_perspective(img: &DynamicImage, strength: f32, rng: &mut Rng) -> DynamicImage {
    let image = img.to_rgb8();
    let (width, height) = (image.width() as f32, image.height() as f32);

    let shift = strength * width;
    let jitter = |rng: &mut Rng| rng.float(0.35, 1.0) * shift;

    let (top_left, top_right, bottom_right, bottom_left) = (
        (jitter(rng), jitter(rng)),
        (width - jitter(rng), jitter(rng)),
        (width - jitter(rng), height - jitter(rng)),
        (jitter(rng), height - jitter(rng)),
    );

    let projection = Projection::from_control_points(
        [(0.0, 0.0), (width, 0.0), (width, height), (0.0, height)],
        [top_left, top_right, bottom_right, bottom_left],
    );

    let Some(projection) = projection else {
        return img.clone();
    };

    let mut out = ImageBuffer::from_pixel(image.width(), image.height(), Rgb([255, 255, 255]));
    warp_into(
        &image,
        &projection,
        Interpolation::Bicubic,
        Rgb([255, 255, 255]),
        &mut out,
    );

    out.into()
}

/// Pose le document sur un support visible : c'est ce qui distingue une photo d'un
/// scan, et ce que la détection d'angle par transformée de Hough accroche à tort.
fn add_background(img: &DynamicImage, rng: &mut Rng) -> DynamicImage {
    let image = img.to_rgb8();
    let margin_x = (image.width() as f32 * rng.float(0.06, 0.16)) as u32;
    let margin_y = (image.height() as f32 * rng.float(0.04, 0.12)) as u32;

    let base = rng.int(70, 140) as u8;
    let mut canvas = ImageBuffer::from_fn(
        image.width() + 2 * margin_x,
        image.height() + 2 * margin_y,
        |x, y| {
            // texture grossière du support, pour ne pas offrir un fond parfaitement uni
            let grain = ((x / 7 + y / 5) % 11) as i32 - 5;
            let level = (base as i32 + grain).clamp(0, 255) as u8;
            Rgb([level, level, (level as u16 * 97 / 100) as u8])
        },
    );

    image::imageops::overlay(&mut canvas, &image, margin_x as i64, margin_y as i64);

    canvas.into()
}

/// Éclairage inégal : dégradé directionnel doublé d'une ombre portée.
fn apply_illumination(img: &DynamicImage, strength: f32, rng: &mut Rng) -> DynamicImage {
    let image = img.to_luma8();
    let (width, height) = (image.width() as f32, image.height() as f32);

    let (dir_x, dir_y) = (rng.float(-1.0, 1.0), rng.float(-1.0, 1.0));
    let (shadow_x, shadow_y) = (rng.float(0.0, width), rng.float(0.0, height));
    let shadow_radius = width.max(height) * rng.float(0.18, 0.42);

    let out = ImageBuffer::from_fn(image.width(), image.height(), |x, y| {
        let (fx, fy) = (x as f32 / width - 0.5, y as f32 / height - 0.5);

        let mut gain = 1.0 - strength * 0.55 * (fx * dir_x + fy * dir_y + 0.5);

        let distance = ((x as f32 - shadow_x).powi(2) + (y as f32 - shadow_y).powi(2)).sqrt();
        if distance < shadow_radius {
            gain *= 1.0 - strength * 0.45 * (1.0 - distance / shadow_radius);
        }

        let value = image.get_pixel(x, y).0[0] as f32 * gain;

        Luma([value.clamp(0.0, 255.0) as u8])
    });

    DynamicImage::ImageLuma8(out)
}

fn add_noise(img: &DynamicImage, sigma: f32, rng: &mut Rng) -> DynamicImage {
    let image = img.to_luma8();

    // somme de deux tirages uniformes : approximation suffisante d'un bruit gaussien
    let out = ImageBuffer::from_fn(image.width(), image.height(), |x, y| {
        let noise = (rng.float(-1.0, 1.0) + rng.float(-1.0, 1.0)) * sigma;
        let value = image.get_pixel(x, y).0[0] as f32 + noise;

        Luma([value.clamp(0.0, 255.0) as u8])
    });

    DynamicImage::ImageLuma8(out)
}

/// Applique une recette complète, dans l'ordre où les défauts apparaissent
/// physiquement : géométrie, puis support, puis lumière, puis capteur.
pub fn apply(img: &DynamicImage, degradation: &Degradation, rng: &mut Rng) -> DynamicImage {
    let mut out = if degradation.is_photo() {
        crop_to_content(img, 0.04)
    } else {
        img.clone()
    };

    // l'orientation de prise de vue précède tout le reste
    out = match degradation.quarter_turns % 4 {
        1 => out.rotate90(),
        2 => out.rotate180(),
        3 => out.rotate270(),
        _ => out,
    };

    if degradation.perspective > 0.0 {
        out = apply_perspective(&out, degradation.perspective, rng);
    }

    if degradation.rotation_deg != 0.0 {
        out = rotate(&out, degradation.rotation_deg.to_radians());
    }

    if degradation.background {
        out = add_background(&out, rng);
    }

    if degradation.illumination > 0.0 {
        out = apply_illumination(&out, degradation.illumination, rng);
    }

    if degradation.blur_sigma > 0.0 {
        out = DynamicImage::ImageLuma8(imageproc::filter::gaussian_blur_f32(
            &out.to_luma8(),
            degradation.blur_sigma,
        ));
    }

    if degradation.noise > 0.0 {
        out = add_noise(&out, degradation.noise, rng);
    }

    out
}

/// Encode en JPEG à la qualité demandée.
pub fn to_jpeg(img: &DynamicImage, quality: u8) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    let gray = img.to_luma8();

    JpegEncoder::new_with_quality(&mut buffer, quality)
        .encode_image(&gray)
        .expect("encodage JPEG");

    buffer.into_inner()
}

pub fn to_png(img: &DynamicImage) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());

    img.to_luma8()
        .write_to(&mut buffer, image::ImageFormat::Png)
        .expect("encodage PNG");

    buffer.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_pdftoppm_resolution_is_marginal() {
        // 150 dpi, la valeur par défaut de pdftoppm, ne laisse qu'une vingtaine de
        // pixels de haut à l'IBAN : c'est la marge dans laquelle le pipeline travaille
        // aujourd'hui sur les PDF scannés.
        assert_eq!(dpi_for_cap_height(30), 273);
        assert_eq!(dpi_for_cap_height(20), 182);
        assert_eq!(dpi_for_cap_height(10), 91);
    }

    #[test]
    fn recipe_names_are_stable_and_readable() {
        assert_eq!(Degradation::default().recipe(), "h30");

        let photo = Degradation {
            cap_height: 14,
            rotation_deg: 3.0,
            perspective: 0.04,
            background: true,
            jpeg_quality: Some(50),
            ..Default::default()
        };

        assert_eq!(photo.recipe(), "h14_rot30_persp4_bg_q50");

        let turned = Degradation {
            quarter_turns: 3,
            ..Default::default()
        };
        assert_eq!(turned.recipe(), "h30_turn270");
        assert!(photo.is_photo());
        assert!(!Degradation::default().is_photo());
    }
}
