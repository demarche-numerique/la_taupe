//! Les blocs d'adresse d'une page, reconstruits à partir des lignes lues par l'OCR.
//!
//! Le recadrage du titulaire était un multiple fixe du code postal : il happait la
//! colonne voisine, coupait un nom qui débordait, montait jusqu'au bloc d'au-dessus.
//! Ici, on part de mots clés — code postal, voie, civilité, libellé — et chaque graine
//! fait croître son bloc de ligne en ligne, tant que la suivante est proche, alignée et
//! de même corps ; une ligne prise par un bloc ne l'est pas par un autre, et une autre
//! ligne de code postal arrête la croissance : deux blocs voisins s'arrêtent l'un contre
//! l'autre (une ligne de partage des eaux). Seuls les blocs qui ont la forme d'un
//! titulaire — une civilité, une forme juridique ou un libellé en tête — sont retenus.
//!
//! On travaille sur les boîtes de lignes de l'OCR, pas sur les pixels : sur les photos
//! réelles, l'encre se binarise mal (un texte flou disparaît, un fond texturé relie
//! tout), alors que le détecteur de lignes tient.

use regex::Regex;

use crate::lines::TextLine;

/// Un morceau de ligne : les mots d'une ligne lue, coupés aux grands blancs — le
/// détecteur fusionne parfois deux colonnes en une seule ligne.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub text: String,
}

impl Segment {
    fn height(&self) -> i32 {
        (self.y1 - self.y0).max(1)
    }
}

/// Un bloc d'adresse : son rectangle et ce qui le désigne.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    /// Hauteur de ligne médiane du bloc.
    pub line: i32,
    /// Indices de ses morceaux de ligne, de bas en haut.
    pub members: Vec<usize>,
    /// Porte un libellé de titulaire.
    pub labelled: bool,
    /// Commence par une civilité ou une forme juridique.
    pub civility: bool,
    /// Porte un mot de domiciliation : l'agence, pas le titulaire.
    pub domiciliation: bool,
    /// Porte une ligne de code postal.
    pub postal: bool,
}

impl Block {
    /// Un libellé ou une civilité le désigne, et rien n'en fait une domiciliation.
    pub fn is_holder_like(&self) -> bool {
        (self.labelled || self.civility) && !self.domiciliation
    }
}

/// Ce que dit une ligne d'elle-même.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Marks {
    postal: bool,
    street: bool,
    civility: bool,
    label: bool,
    domiciliation: bool,
    /// Intitulé d'un champ (« Libellé du sous-compte : ») : la fin d'un bloc d'adresse.
    field: bool,
}

struct Patterns {
    postal: Regex,
    street: Regex,
    civility: Regex,
    legal_form: Regex,
    couple: Regex,
    label: Regex,
    domiciliation: Regex,
}

impl Patterns {
    fn new() -> Self {
        Self {
            // un code postal suivi d'une ville — pas une rangée de chiffres du RIB
            postal: Regex::new(r"(^|\s)(F-)?\d{5}\s*[[:alpha:]]{2,}").unwrap(),
            street: Regex::new(
                r"(?i)\b(rue|avenue|av|bd|boulevard|all[ée]e|chemin|ch|place|pl|impasse|imp|route|rte|quai|cours|square|villa|lieu[- ]dit|r[ée]sidence|r[ée]s|lotissement|lot|chauss[ée]e|faubourg|fbg|passage|parvis|esplanade|sentier|hameau|cit[ée]|domaine|bp|cs|zi|za|zac|b[aâ]t|b[aâ]timent|appartement|appt|apt)\b",
            )
            .unwrap(),
            civility: Regex::new(
                r"(?i)^\s*((m|mr|monsieur)\.?\s*ou\s+)?(m|mr|monsieur|mme|madame|mlle|mle|melle|mademoiselle)(\.\s*|\s+)[[:alpha:]]{2,}",
            )
            .unwrap(),
            // en majuscules seulement, comme au chemin du recadrage
            legal_form: Regex::new(
                r"^\s*(ASSOCIATION|ASSOC\.?|ASS|SASU|SAS|SARL|S\.A\.R\.L\.|EURL|SCI|SCP|SELARL|EARL|GAEC|SCEA|SNC|GIE|SCOP|FONDATION)\s",
            )
            .unwrap(),
            couple: Regex::new(r"^\s*[[:upper:]]+ +[[:upper:]]+ +OU +[[:upper:]]+ +[[:upper:]]+")
                .unwrap(),
            label: Regex::new(r"(?i)(titulaire|intitul[ée]|account owner|b[ée]n[ée]ficiaire)")
                .unwrap(),
            domiciliation: Regex::new(r"(?i)(domiciliation|agence|cadre r[ée]serv[ée])").unwrap(),
        }
    }

    fn marks(&self, text: &str) -> Marks {
        let label = self.label.is_match(text);
        Marks {
            postal: self.postal.is_match(text),
            street: self.street.is_match(text),
            civility: self.civility.is_match(text)
                || self.legal_form.is_match(text)
                || self.couple.is_match(text),
            label,
            domiciliation: self.domiciliation.is_match(text),
            // « Titulaire du compte : » est un libellé, pas la fin du bloc
            field: !label && text.trim_end().ends_with(':'),
        }
    }
}

/// Les morceaux de ligne de la page : chaque ligne coupée là où deux mots sont séparés
/// de plus de deux hauteurs de ligne.
pub fn segments(lines: &[TextLine]) -> Vec<Segment> {
    let mut out = Vec::new();
    for line in lines {
        let height = line.bounding_rect().height().max(1);
        let mut current: Option<Segment> = None;
        for word in line.words() {
            let r = word.bounding_rect();
            let text = word.to_string();
            match current.as_mut() {
                Some(s) if r.left() - s.x1 <= 2 * height => {
                    s.x1 = s.x1.max(r.right());
                    s.y0 = s.y0.min(r.top());
                    s.y1 = s.y1.max(r.bottom());
                    s.text.push(' ');
                    s.text.push_str(&text);
                }
                _ => {
                    out.extend(current.take());
                    current = Some(Segment {
                        x0: r.left(),
                        y0: r.top(),
                        x1: r.right(),
                        y1: r.bottom(),
                        text,
                    });
                }
            }
        }
        out.extend(current);
    }
    out
}

/// Le morceau `s` prolonge-t-il le bloc vers le haut (ou vers le bas, `down`) : proche
/// d'au plus une ligne et demie, de même corps, et aligné — même bord gauche, même bord
/// droit ou même centre, à deux hauteurs de ligne près. Un libellé n'a pas à être
/// aligné : il suffit qu'il chevauche le bloc.
fn extends(block: &Block, s: &Segment, label: bool, down: bool) -> bool {
    let line = block.line;
    let gap = if down {
        s.y0 - block.y1
    } else {
        block.y0 - s.y1
    };
    if gap > line * 3 / 2 || gap < -line / 2 {
        return false;
    }
    let ratio = s.height() as f32 / line as f32;
    if !(0.6..=1.7).contains(&ratio) {
        return false;
    }
    let overlaps = s.x1 > block.x0 && s.x0 < block.x1;
    if label {
        return overlaps;
    }
    let tolerance = 2 * line;
    let aligned = (s.x0 - block.x0).abs() <= tolerance
        || (s.x1 - block.x1).abs() <= tolerance
        || ((s.x0 + s.x1) - (block.x0 + block.x1)).abs() / 2 <= tolerance;
    overlaps && aligned
}

fn grow(block: &mut Block, index: usize, s: &Segment) {
    block.x0 = block.x0.min(s.x0);
    block.y0 = block.y0.min(s.y0);
    block.x1 = block.x1.max(s.x1);
    block.y1 = block.y1.max(s.y1);
    block.members.push(index);
}

fn seed(index: usize, s: &Segment, marks: Marks) -> Block {
    Block {
        x0: s.x0,
        y0: s.y0,
        x1: s.x1,
        y1: s.y1,
        line: s.height(),
        members: vec![index],
        labelled: marks.label,
        civility: marks.civility,
        domiciliation: marks.domiciliation,
        postal: marks.postal,
    }
}

/// Les blocs d'adresse de la page. Chaque ligne de code postal monte jusqu'à une
/// civilité, un libellé, une autre ligne de code postal ou un blanc — six lignes au
/// plus ; puis chaque civilité restée libre — son code postal n'a pas été lu —
/// descend jusqu'à un code postal ou un blanc.
pub fn address_blocks(segments: &[Segment]) -> Vec<Block> {
    const MAX_LINES: usize = 6;
    let patterns = Patterns::new();
    let marks: Vec<Marks> = segments.iter().map(|s| patterns.marks(&s.text)).collect();
    let mut taken = vec![false; segments.len()];
    let mut blocks = Vec::new();

    // de haut en bas : à égalité, le bloc d'au-dessus est servi d'abord
    let mut seeds: Vec<usize> = (0..segments.len()).filter(|&i| marks[i].postal).collect();
    seeds.sort_by_key(|&i| segments[i].y0);

    for &start in &seeds {
        if taken[start] {
            continue;
        }
        taken[start] = true;
        let mut block = seed(start, &segments[start], marks[start]);
        // le morceau libre le plus proche au-dessus du bloc qui le prolonge
        let above = |block: &Block, taken: &[bool]| {
            (0..segments.len())
                .filter(|&i| !taken[i] && segments[i].y1 <= block.y0 + block.line / 2)
                .filter(|&i| extends(block, &segments[i], marks[i].label, false))
                .max_by_key(|&i| segments[i].y1)
                // une autre ligne de code postal : un autre bloc commence
                .filter(|&i| !marks[i].postal && !marks[i].field)
        };
        while block.members.len() < MAX_LINES && !block.civility && !block.labelled {
            let Some(next) = above(&block, &taken) else {
                break;
            };
            taken[next] = true;
            grow(&mut block, next, &segments[next]);
            block.labelled |= marks[next].label;
            block.civility |= marks[next].civility;
            block.domiciliation |= marks[next].domiciliation;
            // un libellé de domiciliation au-dessus : le bloc est celui de l'agence
            if marks[next].domiciliation {
                break;
            }
        }
        // Une civilité ferme le haut du bloc — sauf si une autre la précède à une ou deux
        // lignes : un compte joint sur deux lignes, une raison sociale longue reprise en
        // sigle (« ASSOC. LES AMIS DE… / … / ASSOC LES ADM »). Le bloc monte jusqu'à elle.
        if block.civility && !block.labelled && !block.domiciliation {
            let mut probe = block.clone();
            let mut probe_taken = taken.clone();
            for _ in 0..2 {
                let Some(next) = above(&probe, &probe_taken) else {
                    break;
                };
                if marks[next].label || marks[next].domiciliation {
                    break;
                }
                probe_taken[next] = true;
                grow(&mut probe, next, &segments[next]);
                if marks[next].civility {
                    block = probe.clone();
                    taken.clone_from(&probe_taken);
                }
            }
        }
        blocks.push(block);
    }

    let mut civilities: Vec<usize> = (0..segments.len())
        .filter(|&i| !taken[i] && marks[i].civility)
        .collect();
    civilities.sort_by_key(|&i| segments[i].y0);
    for start in civilities {
        if taken[start] {
            continue;
        }
        taken[start] = true;
        let mut block = seed(start, &segments[start], marks[start]);
        while block.members.len() < MAX_LINES && !block.postal {
            let next = (0..segments.len())
                .filter(|&i| !taken[i] && segments[i].y0 >= block.y1 - block.line / 2)
                .filter(|&i| extends(&block, &segments[i], false, true))
                .min_by_key(|&i| segments[i].y0);
            let Some(next) = next else { break };
            if marks[next].civility || marks[next].label || marks[next].field {
                break;
            }
            taken[next] = true;
            grow(&mut block, next, &segments[next]);
            block.postal |= marks[next].postal;
            block.domiciliation |= marks[next].domiciliation;
        }
        // une civilité seule n'est pas un bloc d'adresse : le dernier recours la prend
        if block.members.len() > 1 {
            blocks.push(block);
        }
    }

    blocks
}

/// Le rectangle à relire autour d'un bloc (x, y, largeur, hauteur) : une demi-ligne de
/// marge au plus de chaque côté, sans entrer dans un morceau de ligne étranger au bloc —
/// un libellé à moitié pris se lit de travers.
pub fn read_rect(block: &Block, segments: &[Segment]) -> (u32, u32, u32, u32) {
    let margin = block.line / 2;
    let others = || {
        segments
            .iter()
            .enumerate()
            .filter(|(i, _)| !block.members.contains(i))
            .map(|(_, s)| s)
    };
    let beside_x = |s: &Segment| s.x1 > block.x0 && s.x0 < block.x1;
    let beside_y = |s: &Segment| s.y1 > block.y0 && s.y0 < block.y1;

    let top = others()
        .filter(|s| beside_x(s) && s.y1 <= block.y0)
        .map(|s| s.y1)
        .fold(block.y0 - margin, i32::max);
    let bottom = others()
        .filter(|s| beside_x(s) && s.y0 >= block.y1)
        .map(|s| s.y0)
        .fold(block.y1 + margin, i32::min);
    let left = others()
        .filter(|s| beside_y(s) && s.x1 <= block.x0)
        .map(|s| s.x1)
        .fold(block.x0 - margin, i32::max);
    let right = others()
        .filter(|s| beside_y(s) && s.x0 >= block.x1)
        .map(|s| s.x0)
        .fold(block.x1 + margin, i32::min);

    let (x, y) = (left.max(0), top.max(0));
    (
        x as u32,
        y as u32,
        (right - x).max(1) as u32,
        (bottom - y).max(1) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str, x0: i32, y0: i32, x1: i32) -> Segment {
        Segment {
            x0,
            y0,
            x1,
            y1: y0 + 20,
            text: text.to_string(),
        }
    }

    /// Deux colonnes côte à côte : l'agence à gauche, le titulaire à droite. Chacune
    /// monte de son code postal à sa tête sans prendre la voisine.
    #[test]
    fn two_columns_grow_their_own_blocks() {
        let page = vec![
            seg("Domiciliation", 100, 100, 300),
            seg("BDF PARIS", 100, 130, 260),
            seg("75001 PARIS", 100, 160, 280),
            seg("Titulaire du compte", 600, 70, 900),
            seg("M MONET CLAUDE", 600, 100, 850),
            seg("12 RUE DES NYMPHEAS", 600, 130, 920),
            seg("27620 GIVERNY", 600, 160, 820),
            seg("CLE RIB", 600, 260, 700),
        ];
        let blocks = address_blocks(&page);

        let holder: Vec<&Block> = blocks.iter().filter(|b| b.is_holder_like()).collect();
        assert_eq!(holder.len(), 1);
        assert_eq!(holder[0].members, vec![6, 5, 4]);
        assert!(holder[0].civility);

        let agency = blocks.iter().find(|b| b.members.contains(&2)).unwrap();
        assert!(agency.domiciliation);
    }

    /// Un code postal plus haut appartient à un autre bloc : la croissance s'y arrête ;
    /// un grand blanc aussi.
    #[test]
    fn growth_stops_at_another_postal_line_or_a_gap() {
        let page = vec![
            seg("MME KAHLO FRIDA", 100, 40, 400),
            seg("44100 NANTES", 100, 70, 300),
            seg("BANQUE DE FRANCE", 100, 100, 400),
            seg("75001 PARIS", 100, 130, 300),
            seg("M MONET CLAUDE", 100, 300, 400),
            seg("27620 GIVERNY", 100, 330, 300),
        ];
        let blocks = address_blocks(&page);

        let paris = blocks.iter().find(|b| b.members[0] == 3).unwrap();
        assert_eq!(paris.members, vec![3, 2]);
        assert!(!paris.is_holder_like());
        let giverny = blocks.iter().find(|b| b.members[0] == 5).unwrap();
        assert_eq!(giverny.members, vec![5, 4]);
    }

    /// Sans code postal lu, une civilité descend jusqu'au bout de son bloc.
    #[test]
    fn a_civility_without_postal_code_grows_downwards() {
        let page = vec![
            seg("M OU MME MONET CLAUDE", 100, 100, 450),
            seg("12 RUE DES NYMPHEAS", 100, 130, 420),
            seg("IBAN FR76 3000 1000 6449 1900 9562 088", 100, 260, 800),
        ];
        let blocks = address_blocks(&page);

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].members, vec![0, 1]);
        assert!(blocks[0].is_holder_like());
    }

    /// Une raison sociale reprise en sigle deux lignes plus bas : le bloc monte jusqu'à
    /// la première forme juridique.
    #[test]
    fn a_second_legal_form_above_extends_the_block() {
        let page = vec![
            seg("ASSOC. LES AMIS DES NYMPHEAS", 100, 40, 500),
            seg("DE GIVERNY", 100, 70, 300),
            seg("ASSOC LES ADN", 100, 100, 350),
            seg("12 RUE DES NYMPHEAS", 100, 130, 420),
            seg("27620 GIVERNY", 100, 160, 300),
        ];
        let blocks = address_blocks(&page);

        assert_eq!(blocks[0].members, vec![4, 3, 2, 1, 0]);
    }

    /// L'intitulé d'un champ ferme le bloc : une civilité seule au-dessus n'en fait pas un.
    #[test]
    fn a_field_label_closes_the_block() {
        let page = vec![
            seg("ASSOCIATION LES NYMPHEAS", 100, 100, 450),
            seg("Libellé du sous-compte :", 100, 130, 420),
        ];
        assert!(address_blocks(&page).is_empty());
    }

    /// La marge s'arrête au bord du libellé voisin.
    #[test]
    fn the_read_rect_does_not_cut_into_a_neighbour() {
        let page = vec![
            seg("Titulaire - Account Owner", 300, 75, 700),
            seg("ASS LES NYMPHEAS", 100, 100, 450),
            seg("27620 GIVERNY", 100, 130, 300),
        ];
        let blocks = address_blocks(&page);
        let block = blocks.iter().find(|b| b.members[0] == 2).unwrap();

        let (_, y, _, _) = read_rect(block, &page);
        assert_eq!(y, 95, "la marge s'arrête sous le libellé");
    }
}
