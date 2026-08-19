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
