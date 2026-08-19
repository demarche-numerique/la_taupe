//! Lignes de texte positionnées : le format d'échange entre le moteur d'OCR et la
//! cascade.
//!
//! Ces types reprennent la surface exacte que la cascade utilisait de la crate `ocrs`,
//! restée seule dépendance après le passage à PP-OCR : une ligne est une suite de
//! caractères positionnés, ses mots sont découpés aux espaces, et chaque élément donne
//! son rectangle englobant.

use regex::Regex;

use crate::shapes::{Anchor, Point};

/// Rectangle aligné sur les axes, en pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    top: i32,
    left: i32,
    height: i32,
    width: i32,
}

impl Rect {
    pub fn from_tlhw(top: i32, left: i32, height: i32, width: i32) -> Self {
        Self {
            top,
            left,
            height,
            width,
        }
    }

    pub fn top(&self) -> i32 {
        self.top
    }

    pub fn left(&self) -> i32 {
        self.left
    }

    pub fn bottom(&self) -> i32 {
        self.top + self.height
    }

    pub fn right(&self) -> i32 {
        self.left + self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    /// Plus petit rectangle couvrant les deux.
    fn union(&self, other: &Rect) -> Rect {
        let top = self.top.min(other.top);
        let left = self.left.min(other.left);
        Rect {
            top,
            left,
            height: self.bottom().max(other.bottom()) - top,
            width: self.right().max(other.right()) - left,
        }
    }
}

/// Un caractère reconnu et son rectangle.
#[derive(Debug, Clone, Copy)]
pub struct TextChar {
    pub char: char,
    pub rect: Rect,
}

/// Une ligne de texte : des caractères positionnés, les espaces compris.
#[derive(Debug, Clone)]
pub struct TextLine {
    chars: Vec<TextChar>,
}

impl TextLine {
    pub fn new(chars: Vec<TextChar>) -> Self {
        Self { chars }
    }

    pub fn bounding_rect(&self) -> Rect {
        bounding_rect(&self.chars)
    }

    /// Les mots de la ligne, découpés aux espaces.
    pub fn words(&self) -> impl Iterator<Item = TextWord<'_>> {
        self.chars
            .split(|c| c.char.is_whitespace())
            .filter(|chars| !chars.is_empty())
            .map(|chars| TextWord { chars })
    }
}

impl std::fmt::Display for TextLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for c in &self.chars {
            write!(f, "{}", c.char)?;
        }
        Ok(())
    }
}

/// Un mot d'une ligne : une vue sur ses caractères.
pub struct TextWord<'a> {
    chars: &'a [TextChar],
}

impl TextWord<'_> {
    pub fn bounding_rect(&self) -> Rect {
        bounding_rect(self.chars)
    }
}

impl std::fmt::Display for TextWord<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for c in self.chars {
            write!(f, "{}", c.char)?;
        }
        Ok(())
    }
}

fn bounding_rect(chars: &[TextChar]) -> Rect {
    chars
        .iter()
        .map(|c| c.rect)
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Rect::from_tlhw(0, 0, 0, 0))
}

pub fn lines_to_text(text_lines: &[TextLine]) -> String {
    text_lines
        .iter()
        // les détections parasites d'un caractère ne font pas une ligne
        .filter(|l| l.to_string().len() > 1)
        .map(|l| l.to_string())
        .collect::<Vec<String>>()
        .join("\n")
}

pub fn extract_anchors(
    text_lines: Vec<TextLine>,
    word_regex: &Regex,
    line_regex: Option<&Regex>,
) -> Vec<Anchor> {
    text_lines
        .iter()
        .filter(|line| {
            if line_regex.is_none() {
                return true;
            }
            line_regex.unwrap().is_match(&line.to_string())
        })
        .flat_map(|line| line.words().collect::<Vec<TextWord>>())
        .filter(|word| word_regex.is_match(&word.to_string()))
        .map(|word| {
            let rect = word.bounding_rect();

            // Les masques sont des multiples de la largeur de l'ancre. Pour un code
            // postal collé à sa ville — « 44800ST » — la largeur du mot entier ferait un
            // masque soixante pour cent trop large, qui empiète sur la colonne voisine :
            // on ne garde que la largeur du motif reconnu. Mais seulement quand le motif
            // est ancré en début de mot et bien plus court que lui : l'ancre d'IBAN
            // « FR76 » suivie du reste doit rester le mot entier, sans quoi le recadrage
            // étroit de l'IBAN devient minuscule et le rate.
            let text = word.to_string();
            let matched = word_regex
                .find(&text)
                .filter(|m| m.start() == 0)
                .map(|m| m.end())
                .unwrap_or(0);
            let total = text.chars().count().max(1);
            let width = rect.width().max(0) as u32;
            let is_postal = matched == 5 && text.chars().take(5).all(|c| c.is_ascii_digit());
            let matched_width = if is_postal && matched < total {
                (width as f32 * matched as f32 / total as f32).round() as u32
            } else {
                width
            };

            Anchor::new(
                Point::new(rect.left().max(0) as u32, rect.top().max(0) as u32),
                Point::new(
                    rect.left().max(0) as u32 + matched_width.max(1),
                    rect.bottom().max(0) as u32,
                ),
            )
        })
        .collect::<Vec<Anchor>>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, y: i32, h: i32) -> TextLine {
        TextLine::new(
            text.chars()
                .enumerate()
                .map(|(i, c)| TextChar {
                    char: c,
                    rect: Rect::from_tlhw(y, 10 + i as i32 * 10, h, 10),
                })
                .collect(),
        )
    }

    #[test]
    fn words_are_split_on_spaces_with_their_rects() {
        let l = line("AB CD", 0, 10);
        let words: Vec<String> = l.words().map(|w| w.to_string()).collect();
        assert_eq!(words, vec!["AB", "CD"]);

        let rects: Vec<Rect> = l.words().map(|w| w.bounding_rect()).collect();
        assert_eq!(rects[0], Rect::from_tlhw(0, 10, 10, 20));
        assert_eq!(rects[1], Rect::from_tlhw(0, 40, 10, 20));
    }
}
