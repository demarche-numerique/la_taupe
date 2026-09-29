use image::DynamicImage;
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub fn bytes_to_img(bytes: Vec<u8>) -> Result<DynamicImage, String> {
    let filetype = tree_magic_mini::from_u8(&bytes);

    match filetype {
        "application/pdf" => {
            let buffer = pdf_to_img_bytes(bytes.clone());
            let img = image::load_from_memory(&buffer).expect("Failed to load image from bytes");
            Ok(img)
        }
        "image/png" | "image/jpeg" => {
            Ok(image::load_from_memory(&bytes).expect("Failed to load image from bytes"))
        }
        _ => Err(format!("Unsupported file type: {}", filetype)),
    }
}

pub fn pdf_bytes_to_string(bytes: Vec<u8>) -> String {
    crate::timing::measure(crate::timing::poppler, || pdf_bytes_to_string_inner(bytes))
}

fn pdf_bytes_to_string_inner(bytes: Vec<u8>) -> String {
    let mut child = Command::new("pdftotext")
        .args(["-layout", "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("Failed to start pdftotext");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(&bytes).expect("Failed to write to stdin");
    });

    let output = child.wait_with_output().expect("Failed to wait on child");

    String::from_utf8_lossy(&output.stdout).to_string()
}

/// Rasterise la première page d'un PDF.
pub fn pdf_to_img_bytes(file: Vec<u8>) -> Vec<u8> {
    pdf_page_to_img_bytes(file, 1)
}

/// Résolution de rastérisation par défaut, celle de `pdftoppm`.
const RASTER_DPI: f32 = 150.0;

/// Plus grand côté, en pixels, d'une page rastérisée.
///
/// Certaines applications — macOS, iOS — mettent un point de page par pixel d'image :
/// un scan de 2550 × 3506 px devient une page de 35 × 49 pouces, dont l'image est
/// affichée à 72 ppi. À 150 dpi, on la rastérisait en 5300 × 7300 px, agrandie deux
/// fois sans un détail de plus : quatre fois les pixels à traiter, et quatre documents
/// de la campagne de production passaient deux minutes sans aboutir. Un A4 à 150 dpi
/// fait 1754 px et un A3 2480 : le plafond ne les touche pas, ni les petites images
/// qu'on agrandit à dessein pour l'OCR.
const MAX_RASTER_SIDE: f32 = 3000.0;

/// Résolution qui rend la page à 150 dpi, sauf si son grand côté dépasserait alors
/// `MAX_RASTER_SIDE` : la résolution est abaissée pour qu'il y tienne.
fn raster_dpi(page_size_pts: Option<(f32, f32)>) -> u32 {
    let Some((width, height)) = page_size_pts else {
        return RASTER_DPI as u32;
    };
    // un point vaut un soixante-douzième de pouce
    let long_side_in = width.max(height) / 72.0;

    if long_side_in * RASTER_DPI <= MAX_RASTER_SIDE {
        RASTER_DPI as u32
    } else {
        ((MAX_RASTER_SIDE / long_side_in).floor() as u32).max(1)
    }
}

/// Taille d'une page, en points, d'après `pdfinfo`.
fn page_size_pts(file: &[u8], page: u32) -> Option<(f32, f32)> {
    let page = page.to_string();
    let mut child = Command::new("pdfinfo")
        .args(["-f", &page, "-l", &page, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut stdin = child.stdin.take()?;
    let owned = file.to_vec();
    std::thread::spawn(move || {
        let _ = stdin.write_all(&owned);
    });

    let output = child.wait_with_output().ok()?;
    parse_page_size(&String::from_utf8_lossy(&output.stdout), &page)
}

/// « Page    3 size:  612 x 792 pts (letter) » → (612, 792).
fn parse_page_size(info: &str, page: &str) -> Option<(f32, f32)> {
    info.lines().find_map(|line| {
        let rest = line.strip_prefix("Page")?.trim_start();
        let rest = rest
            .strip_prefix(page)?
            .trim_start()
            .strip_prefix("size:")?;
        let mut dims = rest.split_whitespace();
        let width = dims.next()?.parse().ok()?;
        (dims.next()? == "x").then_some(())?;
        let height = dims.next()?.parse().ok()?;
        Some((width, height))
    })
}

/// Rasterise une seule page d'un PDF, à 150 dpi sauf pour une page démesurée — voir
/// [`MAX_RASTER_SIDE`].
pub fn pdf_page_to_img_bytes(file: Vec<u8>, page: u32) -> Vec<u8> {
    crate::timing::measure(crate::timing::poppler, || {
        let dpi = raster_dpi(page_size_pts(&file, page)).to_string();
        let page = page.to_string();
        let mut child = Command::new("pdftoppm")
            .args(["-png", "-singlefile", "-r", &dpi, "-f", &page, "-l", &page])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("failed to execute process");

        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        std::thread::spawn(move || {
            stdin.write_all(&file).expect("Failed to write to stdin");
        });

        child
            .wait_with_output()
            .expect("Failed to wait on child")
            .stdout
    })
}

/// Page (à partir de 1) qui porte la plus grande image du PDF, en pixels.
///
/// Un RIB inséré en image dans un document texte — bulletin d'adhésion, notice de
/// virement, règlement de plusieurs pages — n'est lisible qu'en OCR, et rarement seul :
/// un logo l'accompagne, et il n'est pas toujours en première page. La plus grande
/// image est le meilleur candidat ; les vignettes sont ignorées.
pub fn largest_image_page(file: Vec<u8>) -> Option<u32> {
    let mut child = Command::new("pdfimages")
        .args(["-list", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to execute process");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    std::thread::spawn(move || {
        let _ = stdin.write_all(&file);
    });

    let output = child.wait_with_output().ok()?;

    largest_image_page_in_listing(&String::from_utf8_lossy(&output.stdout))
}

fn largest_image_page_in_listing(listing: &str) -> Option<u32> {
    listing
        .lines()
        .skip(2)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            let page = cols.first()?.parse::<u32>().ok()?;
            let width = cols.get(3)?.parse::<u64>().ok()?;
            let height = cols.get(4)?.parse::<u64>().ok()?;
            // un masque ou un filet n'est pas une image de document
            (width >= 200 && height >= 100).then_some((page, width * height))
        })
        .max_by_key(|&(_, area)| area)
        .map(|(page, _)| page)
}

pub fn list_img_in_pdf(file: Vec<u8>) -> usize {
    let mut child = Command::new("pdfimages")
        .args(["-list", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to execute process");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(&file).expect("Failed to write to stdin");
    });

    let output = child.wait_with_output().expect("Failed to wait on child");

    String::from_utf8_lossy(&output.stdout).lines().count() - 2 // Subtract header lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un logo en première page, le RIB en troisième : c'est la page du RIB qu'on veut.
    #[test]
    fn the_page_of_the_largest_image_is_chosen() {
        let listing = "\
page   num  type   width height color comp bpc  enc interp  object ID x-ppi y-ppi size ratio
--------------------------------------------------------------------------------------------
   1     0 image     317   174  rgb     3   8  jpeg   yes        5  0   118   129 9796B 5.9%
   3     1 image     764   376  rgb     3   8  jpeg   yes        7  0   129   130 58.2K 6.9%
   3     2 smask     764   376  gray    1   8  image  no         8  0   129   130 1000B 0.1%
   4     3 image      40    40  rgb     3   8  jpeg   yes        9  0    72    72  900B 5.0%";

        assert_eq!(largest_image_page_in_listing(listing), Some(3));
    }

    /// A4, A3 et petites pages gardent leurs 150 dpi ; une page démesurée voit sa
    /// résolution abaissée pour que son grand côté tienne en 3000 px.
    #[test]
    fn an_oversized_page_is_rasterised_at_a_lower_resolution() {
        // A4 (595 × 842 pts) et A3 (842 × 1191 pts) : inchangés
        assert_eq!(raster_dpi(Some((595.0, 842.0))), 150);
        assert_eq!(raster_dpi(Some((842.0, 1191.0))), 150);
        // petite image agrandie à dessein : inchangée
        assert_eq!(raster_dpi(Some((376.0, 467.0))), 150);
        // scan de 2550 × 3506 px posé à un point par pixel : 3506 pts de haut
        let dpi = raster_dpi(Some((2550.0, 3506.0)));
        assert_eq!(dpi, 61);
        assert!(3506.0 / 72.0 * dpi as f32 <= 3000.0);
        // taille inconnue : le défaut
        assert_eq!(raster_dpi(None), 150);
    }

    #[test]
    fn the_page_size_is_read_from_pdfinfo() {
        let info = "Pages:           2\nPage    1 size:  595.276 x 841.89 pts (A4)\nPage    2 size:  2550 x 3506 pts\n";
        assert_eq!(parse_page_size(info, "1"), Some((595.276, 841.89)));
        assert_eq!(parse_page_size(info, "2"), Some((2550.0, 3506.0)));
        assert_eq!(parse_page_size(info, "3"), None);
    }

    /// Le PDF de test : un RIB fictif en image de 2165 × 740 px posée à 72 ppi, soit
    /// une page de 30 pouces de large, rendue en 4511 px à 150 dpi. Le rendu reste
    /// sous le plafond.
    #[test]
    fn a_page_declared_at_72_ppi_stays_under_the_ceiling() {
        let pdf = std::fs::read("tests/fixtures/rib/image_declared_72ppi.pdf").unwrap();
        let png = pdf_page_to_img_bytes(pdf, 1);
        let img = image::load_from_memory(&png).unwrap();

        assert!(img.width().max(img.height()) <= 3000);
    }

    #[test]
    fn thumbnails_alone_give_no_page() {
        let listing = "\
page   num  type   width height color comp bpc  enc interp  object ID x-ppi y-ppi size ratio
--------------------------------------------------------------------------------------------
   1     0 image      64    32  rgb     3   8  jpeg   yes        5  0    72    72  900B 5.0%";

        assert_eq!(largest_image_page_in_listing(listing), None);
    }
}
