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

pub fn pdf_to_img_bytes(file: Vec<u8>) -> Vec<u8> {
    crate::timing::measure(crate::timing::poppler, || pdf_to_img_bytes_inner(file))
}

fn pdf_to_img_bytes_inner(file: Vec<u8>) -> Vec<u8> {
    let mut child = Command::new("pdftoppm")
        .args(["-png", "-singlefile"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to execute process");

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    std::thread::spawn(move || {
        stdin.write_all(&file).expect("Failed to write to stdin");
    });

    let output = child.wait_with_output().expect("Failed to wait on child");
    output.stdout
}

/// Rasterise une seule page d'un PDF, à la résolution par défaut de `pdftoppm`.
pub fn pdf_page_to_img_bytes(file: Vec<u8>, page: u32) -> Vec<u8> {
    crate::timing::measure(crate::timing::poppler, || {
        let page = page.to_string();
        let mut child = Command::new("pdftoppm")
            .args(["-png", "-singlefile", "-f", &page, "-l", &page])
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

    #[test]
    fn thumbnails_alone_give_no_page() {
        let listing = "\
page   num  type   width height color comp bpc  enc interp  object ID x-ppi y-ppi size ratio
--------------------------------------------------------------------------------------------
   1     0 image      64    32  rgb     3   8  jpeg   yes        5  0    72    72  900B 5.0%";

        assert_eq!(largest_image_page_in_listing(listing), None);
    }
}
