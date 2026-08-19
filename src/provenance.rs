//! Trace du chemin suivi pour extraire un RIB.
//!
//! Le pipeline enchaîne plusieurs stratégies jusqu'à ce que l'une aboutisse, sans
//! jamais dire laquelle. Sans cette trace, impossible de savoir quelles branches
//! servent réellement, donc lesquelles méritent d'être améliorées ou retirées.
//!
//! Ne contient que des étiquettes et des grandeurs géométriques : aucun texte reconnu,
//! de sorte qu'une trace puisse être publiée sans divulguer le contenu du document.

/// Branche d'aiguillage retenue par `analysis::vec_to_rib`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Texte extrait du PDF, sans OCR.
    PdfText,
    /// PDF rasterisé faute de texte exploitable.
    PdfImage,
    /// Image fournie telle quelle.
    Image,
    /// Fichier texte brut.
    PlainText,
}

/// Moteur ayant fourni l'ancre de localisation de l'IBAN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorSource {
    Ocrs,
    Tesseract,
}

/// Stratégie ayant effectivement produit l'IBAN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// `pdftotext`, sans OCR.
    PdfText,
    /// OCR de la page entière par ocrs.
    OcrsPage,
    /// Recadrage autour d'une ancre ocrs.
    OcrsCrop,
    /// Recadrage étroit, pour les IBAN espacés de façon inhabituelle.
    OcrsNarrowCrop,
    /// Recadrage autour d'une ancre tesseract, après redressement éventuel.
    TessCrop,
}

impl Route {
    pub fn as_str(&self) -> &'static str {
        match self {
            Route::PdfText => "pdf_text",
            Route::PdfImage => "pdf_image",
            Route::Image => "image",
            Route::PlainText => "plain_text",
        }
    }
}

impl AnchorSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            AnchorSource::Ocrs => "ocrs",
            AnchorSource::Tesseract => "tess",
        }
    }
}

impl Engine {
    pub fn as_str(&self) -> &'static str {
        match self {
            Engine::PdfText => "pdf_text",
            Engine::OcrsPage => "ocrs:page",
            Engine::OcrsCrop => "ocrs:crop",
            Engine::OcrsNarrowCrop => "ocrs:narrow",
            Engine::TessCrop => "tess:crop",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Provenance {
    pub route: Option<Route>,
    pub anchor: Option<AnchorSource>,
    /// Hauteur de l'ancre en pixels : c'est la grandeur qui décide si le texte est
    /// assez grand pour être reconnu.
    pub anchor_height: Option<u32>,
    /// Inclinaison détectée par tesseract, en degrés.
    pub angle_deg: Option<f32>,
    pub engine: Option<Engine>,
    /// Vrai si le résultat vient de la seconde passe, sur image nettoyée.
    pub second_pass: bool,
    pub image_width: u32,
    pub image_height: u32,
}

impl Provenance {
    pub fn route(route: Route) -> Self {
        Provenance {
            route: Some(route),
            ..Default::default()
        }
    }
}
