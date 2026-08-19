//! Compteurs de temps par étape, à l'échelle du thread.
//!
//! Le banc dit combien de temps prend un document ; il ne dit pas où. Ces compteurs
//! ventilent la durée entre les moteurs et le prétraitement — c'est ce qui décide où
//! porter l'effort de latence.

use std::cell::RefCell;
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Timings {
    /// Reconnaissance PP-OCR (détection + reconnaissance).
    pub ocr: Duration,
    pub ocr_calls: u32,
    /// Sous-processus tesseract, hocr et texte.
    pub tesseract: Duration,
    pub tesseract_calls: u32,
    /// Nettoyage, rotation, redimensionnement.
    pub preprocess: Duration,
    /// Rastérisation et extraction de texte des PDF.
    pub poppler: Duration,
}

thread_local! {
    static CURRENT: RefCell<Timings> = RefCell::new(Timings::default());
}

pub fn reset() {
    CURRENT.with(|t| *t.borrow_mut() = Timings::default());
}

pub fn snapshot() -> Timings {
    CURRENT.with(|t| *t.borrow())
}

/// Mesure une étape et l'impute à un compteur.
pub fn measure<T>(bucket: fn(&mut Timings, Duration), f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    CURRENT.with(|t| bucket(&mut t.borrow_mut(), elapsed));
    result
}

pub fn ocr(t: &mut Timings, d: Duration) {
    t.ocr += d;
    t.ocr_calls += 1;
}

pub fn tesseract(t: &mut Timings, d: Duration) {
    t.tesseract += d;
    t.tesseract_calls += 1;
}

pub fn preprocess(t: &mut Timings, d: Duration) {
    t.preprocess += d;
}

pub fn poppler(t: &mut Timings, d: Duration) {
    t.poppler += d;
}
