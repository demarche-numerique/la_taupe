//! Photos synthétiques dérivées de documents existants.
//!
//! Le générateur ne connaît que ses propres gabarits ; les mises en page réelles, bien
//! plus variées, n'existent qu'en PDF natifs ou en scans. On les photographie donc
//! virtuellement, avec les mêmes recettes que le générateur, en gardant leur vérité
//! terrain. Un même document décliné en propre et en photo mesure l'effet de la seule
//! prise de vue, à contenu identique.
//!
//! La police des documents réels n'étant pas connue, la résolution n'est pas pilotée par
//! la hauteur de capitale de l'IBAN mais par le cadrage : le document imprimé occupe
//! `width` pixels de large, comme sur une photo prise au téléphone puis réduite.

use std::fs;
use std::path::Path;
use std::process::Command;

use image::{imageops::FilterType, DynamicImage};

use super::degrade::{self, Degradation};
use super::rng::Rng;

/// Résolution de rastérisation, avant recadrage et mise à la largeur voulue : assez haute
/// pour que la réduction, et non la rastérisation, fixe la finesse du texte.
const RASTER_DPI: u32 = 300;

pub struct PhotoRecipe {
    pub name: &'static str,
    /// Largeur du document imprimé sur la photo, en pixels.
    pub width: u32,
    pub degradation: Degradation,
}

fn photo() -> Degradation {
    Degradation {
        background: true,
        illumination: 0.4,
        blur_sigma: 0.5,
        jpeg_quality: Some(70),
        ..Default::default()
    }
}

/// Trois prises de vue : ordinaire, bâclée, et appareil tourné — un quart des photos
/// réelles arrivent pivotées d'un quart de tour.
pub fn recipes() -> Vec<PhotoRecipe> {
    vec![
        PhotoRecipe {
            name: "photo",
            width: 1800,
            degradation: photo(),
        },
        PhotoRecipe {
            name: "photo_dure",
            width: 1200,
            degradation: Degradation {
                rotation_deg: 4.0,
                perspective: 0.05,
                blur_sigma: 0.8,
                noise: 8.0,
                illumination: 0.7,
                jpeg_quality: Some(50),
                ..photo()
            },
        },
        PhotoRecipe {
            name: "photo_turn90",
            width: 1800,
            degradation: Degradation {
                quarter_turns: 1,
                ..photo()
            },
        },
    ]
}

/// Page (à partir de 1) qui porte l'IBAN, d'après la couche texte ; la première sinon.
fn iban_page(path: &Path, iban: &str) -> u32 {
    let Ok(output) = Command::new("pdftotext").arg(path).arg("-").output() else {
        return 1;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let wanted: String = iban.chars().filter(|c| !c.is_whitespace()).collect();

    if wanted.is_empty() {
        return 1;
    }

    text.split('\u{c}')
        .position(|page| {
            let compact: String = page.chars().filter(|c| !c.is_whitespace()).collect();
            compact.contains(&wanted)
        })
        .map(|i| i as u32 + 1)
        .unwrap_or(1)
}

fn load(path: &Path, iban: &str) -> Option<DynamicImage> {
    let bytes = fs::read(path).ok()?;

    if bytes.starts_with(b"%PDF") {
        Some(degrade::rasterize(
            &bytes,
            RASTER_DPI,
            iban_page(path, iban),
        ))
    } else {
        image::load_from_memory(&bytes).ok()
    }
}

/// Photographie un document : cadrage sur la zone imprimée, mise à la largeur, défauts
/// de prise de vue, compression.
pub fn photograph(img: &DynamicImage, recipe: &PhotoRecipe, rng: &mut Rng) -> Vec<u8> {
    let framed = degrade::crop_to_content(img, 0.04);
    let height = (framed.height() as f32 * recipe.width as f32 / framed.width() as f32) as u32;
    let sized = framed.resize_exact(recipe.width, height.max(1), FilterType::Triangle);

    let shot = degrade::apply(&sized, &recipe.degradation, rng);

    degrade::to_jpeg(&shot, recipe.degradation.jpeg_quality.unwrap_or(85))
}

fn column(header: &[&str], name: &str) -> Option<usize> {
    header.iter().position(|h| *h == name)
}

/// Dérive un corpus photo de `src` selon sa vérité `truth` (format du banc) et l'écrit
/// dans `out` avec sa propre vérité. Renvoie le nombre de photos écrites.
pub fn write(src: &Path, truth: &Path, out: &Path, seed: u64) -> std::io::Result<usize> {
    fs::create_dir_all(out)?;

    let content = fs::read_to_string(truth)?;
    let mut lines = content.lines();
    let header: Vec<&str> = lines.next().unwrap_or_default().split(';').collect();
    let invalid =
        |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_string());
    let (file, iban, bic, holder) = (
        column(&header, "file").ok_or_else(|| invalid("colonne file absente"))?,
        column(&header, "iban").ok_or_else(|| invalid("colonne iban absente"))?,
        column(&header, "bic").ok_or_else(|| invalid("colonne bic absente"))?,
        column(&header, "account_holder")
            .ok_or_else(|| invalid("colonne account_holder absente"))?,
    );
    let src_col = column(&header, "src");

    let mut rng = Rng::new(seed);
    let mut derived = String::from("file;iban;bic;account_holder;src;recipe;expect\n");
    let mut written = 0;

    for line in lines.filter(|l| !l.trim().is_empty()) {
        let fields: Vec<&str> = line.split(';').collect();
        let field = |i: usize| fields.get(i).copied().unwrap_or("");
        let name = field(file);

        let Some(img) = load(&src.join(name), field(iban)) else {
            eprintln!("ignoré, illisible : {}", name);
            continue;
        };

        let stem = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(name);

        for recipe in recipes() {
            let target = format!("{}__{}.jpg", stem, recipe.name);
            fs::write(out.join(&target), photograph(&img, &recipe, &mut rng))?;

            derived.push_str(&format!(
                "{};{};{};{};{};{};ok\n",
                target,
                field(iban),
                field(bic),
                field(holder),
                src_col.map(field).unwrap_or(""),
                recipe.name
            ));
            written += 1;
        }
    }

    fs::write(out.join("truth.csv"), derived)?;

    Ok(written)
}
