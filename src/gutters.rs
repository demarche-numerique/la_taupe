//! Les colonnes voisines d'un bloc, d'après ses blancs.
//!
//! Le recadrage du titulaire est un multiple fixe de l'ancre du code postal : dix
//! largeurs, neuf lignes. Rien ne le borne au bloc imprimé : il happe la colonne voisine,
//! qui se colle aux lignes du titulaire. Une gouttière — une bande verticale sans encre,
//! large de plusieurs caractères — sépare deux colonnes : on la cherche de part et
//! d'autre du code postal, et ce qui est au-delà n'est pas le titulaire.
//!
//! Délimiter tout le bloc par l'étalement de l'encre (RLSA) a été essayé : sur les photos
//! réelles, l'encre se binarise mal — un texte flou n'y paraît pas et le nom sortait du
//! bloc, un fond texturé reliait tout. Les gouttières, elles, tiennent : un bruit qui les
//! comble fait seulement renoncer au bornage.

use image::{DynamicImage, GrayImage};
use imageproc::contrast::adaptive_threshold;

use crate::shapes::Anchor;

/// Abscisses (gauche, droite) entre lesquelles se tient le bloc du code postal, dans la
/// page : chacune s'arrête une demi-ligne avant la colonne voisine. `None` quand aucune
/// colonne voisine n'est séparée par une gouttière — rien à borner.
pub fn column_span(img: &DynamicImage, anchor: &Anchor) -> Option<(u32, u32)> {
    let line = anchor.height.max(1);
    let left = anchor.top_left.x.saturating_sub(anchor.width * 6);
    let top = anchor.top_left.y.saturating_sub(line * 9);
    let right = (anchor.top_left.x + anchor.width * 12).min(img.width());
    let bottom = (anchor.bottom_right.y + line / 4).min(img.height());
    if right <= left || bottom <= top {
        return None;
    }

    let window = img
        .crop_imm(left, top, right - left, bottom - top)
        .to_luma8();
    let mut ink = ink_of(&window, line);
    erase_rules(&mut ink, line);

    let seed = (
        anchor.top_left.x - left,
        anchor.top_left.y - top,
        anchor.bottom_right.x.min(right) - left,
        anchor.bottom_right.y.min(bottom) - top,
    );
    let (lo, hi) = column_limits(&ink, seed, line);
    let bounded = lo.is_some() || hi.is_some();

    bounded.then(|| {
        (
            left + lo.unwrap_or(0) as u32,
            left + hi.unwrap_or(ink.width.saturating_sub(1)) as u32,
        )
    })
}

/// Une grille de booléens, vrai pour l'encre.
struct Ink {
    width: usize,
    height: usize,
    cells: Vec<bool>,
}

impl Ink {
    fn get(&self, x: usize, y: usize) -> bool {
        self.cells[y * self.width + x]
    }

    fn set(&mut self, x: usize, y: usize, value: bool) {
        self.cells[y * self.width + x] = value;
    }
}

/// L'encre : plus sombre que son voisinage d'une ligne de rayon. Un seuil unique
/// noircit l'ombre d'une photo ; le fond uniforme reste blanc : il faut `CONTRAST`
/// niveaux d'écart.
fn ink_of(gray: &GrayImage, line: u32) -> Ink {
    const CONTRAST: i32 = 15;
    let binary = adaptive_threshold(gray, line.max(2), CONTRAST);
    Ink {
        width: gray.width() as usize,
        height: gray.height() as usize,
        cells: binary.pixels().map(|p| p.0[0] == 0).collect(),
    }
}

/// Efface les filets d'un tableau : un trait plus long que huit lignes de texte, ou
/// plus haut que deux et demie, n'est pas une lettre — un cadre comblerait la gouttière
/// qu'il traverse.
fn erase_rules(ink: &mut Ink, line: u32) {
    let long = (line * 8) as usize;
    let tall = (line * 5 / 2) as usize;

    for y in 0..ink.height {
        let mut x = 0;
        while x < ink.width {
            let start = x;
            while x < ink.width && ink.get(x, y) {
                x += 1;
            }
            if x - start > long {
                (start..x).for_each(|i| ink.set(i, y, false));
            }
            x += 1;
        }
    }
    for x in 0..ink.width {
        let mut y = 0;
        while y < ink.height {
            let start = y;
            while y < ink.height && ink.get(x, y) {
                y += 1;
            }
            if y - start > tall {
                (start..y).for_each(|i| ink.set(x, i, false));
            }
            y += 1;
        }
    }
}

/// Limites de la colonne du code postal : de part et d'autre de l'ancre, on suit
/// l'encre de la bande qui va de trois lignes au-dessus du code postal à sa base — le
/// bloc voisin y est déjà ; après une gouttière de plus de deux lignes, la première
/// encre est la colonne voisine, et la limite se pose une demi-ligne avant elle. On
/// coupe au bord de la colonne voisine, pas au bord du bloc : un nom plus long que son
/// adresse garde toute sa ligne.
fn column_limits(
    ink: &Ink,
    (sx0, sy0, sx1, sy1): (u32, u32, u32, u32),
    line: u32,
) -> (Option<usize>, Option<usize>) {
    let gap = (line * 2) as usize;
    let margin = (line / 2) as usize;
    let band = (sy0.saturating_sub(line * 3) as usize)..(sy1 as usize).min(ink.height);
    let occupied = |x: usize| band.clone().any(|y| ink.get(x, y));
    let (sx0, sx1) = (
        sx0 as usize,
        (sx1 as usize).min(ink.width.saturating_sub(1)),
    );

    let mut lo = None;
    let mut empty = 0;
    for x in (0..sx0).rev() {
        if occupied(x) {
            if empty > gap {
                lo = Some((x + 1 + margin).min(sx0));
                break;
            }
            empty = 0;
        } else {
            empty += 1;
        }
    }

    let mut hi = None;
    empty = 0;
    for x in sx1 + 1..ink.width {
        if occupied(x) {
            if empty > gap {
                hi = Some(x.saturating_sub(1 + margin).max(sx1));
                break;
            }
            empty = 0;
        } else {
            empty += 1;
        }
    }

    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::Point;
    use image::Luma;

    fn page(width: u32, height: u32, texts: &[(u32, u32, u32, u32)]) -> GrayImage {
        let mut img = GrayImage::from_pixel(width, height, Luma([255]));
        for &(x, y, w, h) in texts {
            // des « lettres » : des barres de 6 pixels séparées de 4
            for dx in (0..w).filter(|dx| dx % 10 < 6) {
                for dy in 0..h {
                    img.put_pixel(x + dx, y + dy, Luma([0]));
                }
            }
        }
        img
    }

    /// Trois lignes à gauche — nom, voie, code postal — et une colonne à droite au-delà
    /// d'une large gouttière : la limite droite tombe dans la gouttière.
    #[test]
    fn the_span_stops_at_the_gutter_before_the_next_column() {
        let img = page(
            1200,
            600,
            &[
                (100, 300, 300, 20),
                (100, 330, 260, 20),
                (100, 360, 200, 20),
                (700, 300, 300, 20),
                (700, 330, 300, 20),
            ],
        );
        let postal = Anchor::new(Point::new(100, 360), Point::new(160, 380));

        let (lo, hi) = column_span(&DynamicImage::ImageLuma8(img), &postal).unwrap();
        assert_eq!(lo, 0, "rien à gauche : pas de limite");
        assert!(
            (400..700).contains(&hi),
            "la limite droite est dans la gouttière : {hi}"
        );
    }

    /// Un titre en travers des deux colonnes, au-dessus de la bande, ne comble pas la
    /// gouttière ; un nom plus long que son adresse reste en deçà de la limite.
    #[test]
    fn a_heading_across_both_columns_does_not_join_them() {
        let img = page(
            1400,
            600,
            &[
                (100, 200, 1000, 20), // titre
                (100, 270, 300, 20),
                (100, 300, 300, 20),
                (100, 330, 200, 20),
                (700, 270, 450, 20), // nom long
                (700, 300, 250, 20),
                (700, 330, 200, 20),
            ],
        );
        let postal = Anchor::new(Point::new(700, 330), Point::new(760, 350));

        let (lo, hi) = column_span(&DynamicImage::ImageLuma8(img), &postal).unwrap();
        assert!(
            (400..700).contains(&lo),
            "la colonne de gauche reste dehors : {lo}"
        );
        assert!(hi >= 1150, "le nom long reste en deçà : {hi}");
    }

    /// Le cadre d'un tableau ne comble pas la gouttière qu'il traverse ; sans colonne
    /// voisine, rien à borner.
    #[test]
    fn a_table_frame_does_not_fill_the_gutter() {
        let mut img = page(
            1200,
            600,
            &[
                (100, 300, 300, 20),
                (100, 330, 200, 20),
                (700, 300, 300, 20),
            ],
        );
        for x in 50..1100 {
            img.put_pixel(x, 280, Luma([0]));
            img.put_pixel(x, 400, Luma([0]));
        }
        let postal = Anchor::new(Point::new(100, 330), Point::new(160, 350));
        let (_, hi) = column_span(&DynamicImage::ImageLuma8(img), &postal).unwrap();
        assert!(hi < 700, "le cadre n'a pas comblé la gouttière : {hi}");

        let alone = page(1200, 600, &[(100, 300, 300, 20), (100, 330, 200, 20)]);
        assert_eq!(column_span(&DynamicImage::ImageLuma8(alone), &postal), None);
    }
}
