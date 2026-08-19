use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::file_utils::{list_img_in_pdf, pdf_to_img_bytes};
use crate::provenance::{Engine, Provenance, Route};
use crate::rib::Rib;
use crate::{
    datamatrix::fetch_datamatrix,
    file_utils::{bytes_to_img, pdf_bytes_to_string},
    ocr::image_bytes_to_rib_traced,
    twoddoc::{ddoc::Ddoc, parse},
};

#[derive(Deserialize, Serialize, Debug)]
#[serde(tag = "hint")]
pub enum Analysis {
    #[serde(rename = "2ddoc_and_rib")]
    DdocAndRib {
        #[serde(rename = "2ddoc")]
        ddoc: Option<Ddoc>,
        rib: Option<Rib>,
    },
    #[serde(rename = "rib")]
    Rib { rib: Option<Rib> },
    #[serde(rename = "2ddoc")]
    Ddoc {
        #[serde(rename = "2ddoc")]
        ddoc: Option<Ddoc>,
    },
}

#[derive(Deserialize, Copy, Clone, Debug)]
pub enum Hint {
    #[serde(rename = "type")]
    Type(Type),
}

#[derive(Deserialize, Copy, Clone, Debug)]
pub enum Type {
    #[serde(rename = "rib")]
    Rib,
    #[serde(rename = "2ddoc")]
    Twoddoc,
}

fn vec_to_rib(content: Vec<u8>, name: &str) -> Result<Option<Rib>, String> {
    vec_to_rib_traced(content, name, &mut Provenance::default())
}

/// Même aiguillage, en consignant la branche empruntée et la stratégie qui a abouti.
/// Une seule implémentation, pour que la mesure porte sur le code de production.
pub fn vec_to_rib_traced(
    content: Vec<u8>,
    name: &str,
    provenance: &mut Provenance,
) -> Result<Option<Rib>, String> {
    let filetype = tree_magic_mini::from_u8(&content);

    if filetype == "application/pdf" {
        let string_rib = pdf_bytes_to_string(content.clone());

        if !string_rib.trim().is_empty() {
            provenance.route = Some(Route::PdfText);

            let rib = Rib::parse(string_rib);
            if rib.is_some() {
                provenance.engine = Some(Engine::PdfText);
                Ok(rib)
            // if there is only one image in PDF, it could be a scan of a RIB
            // with some poorly parse text.
            // don't try it for all as it is costly
            } else if list_img_in_pdf(content.clone()) == 1 {
                provenance.route = Some(Route::PdfImage);

                let img = pdf_to_img_bytes(content);
                Ok(image_bytes_to_rib_traced(img, name, provenance))
            } else {
                Ok(None)
            }
        } else {
            provenance.route = Some(Route::PdfImage);

            let img = pdf_to_img_bytes(content);
            Ok(image_bytes_to_rib_traced(img, name, provenance))
        }
    } else if filetype == "image/png" || filetype == "image/jpeg" {
        provenance.route = Some(Route::Image);

        Ok(image_bytes_to_rib_traced(content, name, provenance))
    } else if filetype == "text/plain" {
        provenance.route = Some(Route::PlainText);

        let string_rib = String::from_utf8(content)
            .map_err(|_| "Failed to convert bytes to string".to_string())?;

        let rib = Rib::parse(string_rib);
        if rib.is_some() {
            provenance.engine = Some(Engine::PdfText);
        }

        Ok(rib)
    } else {
        Err(format!("Unsupported file type: {}", filetype))
    }
}

fn vec_to_ddoc(content: Vec<u8>) -> Result<Option<Ddoc>, String> {
    let img = bytes_to_img(content)?;

    if let Some(datamatrix) = fetch_datamatrix(img) {
        Ok(parse(&datamatrix))
    } else {
        Ok(None)
    }
}

impl TryFrom<(Vec<u8>, Option<Hint>, &str)> for Analysis {
    type Error = String;

    fn try_from((content, hint, name): (Vec<u8>, Option<Hint>, &str)) -> Result<Self, String> {
        match hint {
            Some(Hint::Type(Type::Rib)) => {
                let rib = vec_to_rib(content, name)?;

                Ok(Analysis::Rib { rib })
            }
            Some(Hint::Type(Type::Twoddoc)) => {
                let ddoc = vec_to_ddoc(content)?;

                Ok(Analysis::Ddoc { ddoc })
            }
            None => {
                let rib = vec_to_rib(content.clone(), name).unwrap_or(None);
                let ddoc = vec_to_ddoc(content).unwrap_or(None);

                Ok(Analysis::DdocAndRib { ddoc, rib })
            }
        }
    }
}

impl TryFrom<(&Path, Option<Hint>)> for Analysis {
    type Error = String;

    fn try_from((file_path, hint): (&Path, Option<Hint>)) -> Result<Self, String> {
        let base_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown");
        let content =
            std::fs::read(file_path).map_err(|e| format!("Failed to read file: {}", e))?;
        Analysis::try_from((content, hint, base_name))
    }
}
