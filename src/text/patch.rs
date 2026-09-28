use regex::Regex;

#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    inner_lines: Vec<String>,
    pub context_lines: Vec<String>,
}

impl Patch {
    pub fn extract(
        text: &[&str],
        index: usize,
        stop: &Regex,
        start: usize,
        end: usize,
        up: bool,
        nb_line: usize,
    ) -> Self {
        let mut content_lines = Vec::new();
        let mut context_lines = Vec::new();

        // compute real start and end from the base
        let (_content, _before, left, right) = complete(text[index], start, end).unwrap();

        if up {
            // bord gauche le plus à gauche atteint par le bloc : un bloc aligné à droite
            // a des lignes plus longues que celle du code postal
            let mut block_left = left;
            for i in (index.saturating_sub(6)..=index).rev() {
                if let Some((content, before, line_left, _)) = complete(text[i], left, right) {
                    let should_stop = stop.is_match(&content) || stop.is_match(&before);
                    block_left = block_left.min(line_left);

                    content_lines.insert(0, content);
                    context_lines.insert(0, before);

                    if should_stop {
                        break;
                    }
                } else if let Some(label) = label_over_block(text[i], stop, block_left, right) {
                    // Le libellé posé au-dessus d'un bloc aligné à droite commence plus à
                    // gauche que le code postal et s'arrête avant sa colonne : la ligne
                    // était ignorée, et le bloc — « Intitulé du compte » pourtant — restait
                    // sans type, battu par celui de l'agence voisine. Il le désigne et
                    // le ferme.
                    context_lines.insert(0, label);
                    break;
                }
            }
        } else {
            let mut started = false;
            for (offset, line) in text
                .iter()
                .take((index + nb_line).min(text.len()))
                .skip(index)
                .enumerate()
            {
                // Sous le libellé, une ligne blanche dans la colonne du libellé mais
                // occupée ailleurs — la domiciliation, à droite — ferme le bloc : sans
                // quoi `complete` s'élargit jusqu'au texte voisin et le bloc glisse dans
                // l'autre colonne. Seulement une fois le titulaire commencé : avant, le
                // nom peut être décalé sous un libellé posé plus à gauche.
                let blank = column_is_blank(line, left, right);
                if offset > 0 && blank && started {
                    break;
                }
                if offset > 0 && !blank {
                    started = true;
                }
                if let Some((content, before, _, _)) = complete(line, left, right) {
                    let should_stop = stop.is_match(&content) || stop.is_match(&before);

                    content_lines.push(content);
                    context_lines.push(before);

                    if should_stop {
                        break;
                    }
                }
            }
        }

        Patch {
            inner_lines: content_lines,
            context_lines,
        }
    }

    pub fn lines(&self) -> Vec<String> {
        self.inner_lines
            .iter()
            .map(|line| line.trim().to_string())
            .filter(|line| !line.is_empty())
            .collect()
    }
}

/// Libellé de `stop` sur la ligne dont l'étendue chevauche les colonnes du bloc.
fn label_over_block(line: &str, stop: &Regex, left: usize, right: usize) -> Option<String> {
    stop.find_iter(line).find_map(|m| {
        let start = line[..m.start()].chars().count();
        let end = start + m.as_str().chars().count();
        (end > left && start <= right).then(|| m.as_str().to_string())
    })
}

/// Vrai si la ligne n'a que des blancs entre les colonnes `left` et `right` incluses.
fn column_is_blank(line: &str, left: usize, right: usize) -> bool {
    line.chars()
        .skip(left)
        .take(right.saturating_sub(left) + 1)
        .all(char::is_whitespace)
}

fn complete(line: &str, start: usize, end: usize) -> Option<(String, String, usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let left = left_complete(line, start, end)?;
    let right = right_complete(line, left, end)?;
    Some((
        String::from_iter(&chars[left..=right]),
        String::from_iter(&chars[..left]),
        left,
        right,
    ))
}

fn left_complete(line: &str, mut start: usize, mut end: usize) -> Option<usize> {
    if end >= line.chars().count() {
        end = line.chars().count().saturating_sub(1);
    }

    if start >= line.chars().count() || start > end {
        return None;
    }

    let chars: Vec<char> = line.chars().collect();

    while start > 0 && !chars[start..=end].starts_with(&[' ', ' ']) {
        start -= 1;
    }

    if chars[start..=end].starts_with(&[' ', ' ']) {
        start += 2;
    } else if chars[start..=end].starts_with(&[' ']) {
        start += 1;
    }

    Some(start)
}

pub fn right_complete(line: &str, start: usize, mut end: usize) -> Option<usize> {
    if end >= line.chars().count() {
        end = line.chars().count().saturating_sub(1);
    }

    if start >= line.chars().count() || start > end {
        return None;
    }

    let chars: Vec<char> = line.chars().collect();

    while end < line.chars().count() - 1 && !chars[start..=end].ends_with(&[' ', ' ']) {
        end += 1;
    }

    if chars[start..=end].ends_with(&[' ', ' ']) {
        end -= 2;
    } else if chars[start..=end].ends_with(&[' ']) {
        end -= 1;
    }

    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract() {
        let stop = Regex::new(r"(?i)(titulaire)").unwrap();

        let text = vec![
            "bla bla",
            "titulaire : M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44100 NANTES",
        ];

        let patch = Patch::extract(&text, 3, &stop, 0, 11, true, 3);

        let result = vec![
            "titulaire : M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44100 NANTES",
        ];

        assert_eq!(patch.inner_lines, result);

        let text = vec![
            "bla bla    M OU MME MATISSE HENRI",
            "bla bla    51 RUE BERNARD ROY",
            "bla bla    44100 NANTES",
        ];

        let result = vec![
            "M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44100 NANTES",
        ];

        let patch = Patch::extract(&text, 2, &stop, 10, 17, true, 3);

        assert_eq!(patch.inner_lines, result);

        let text = vec![
            "M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44166 NANTES SUR LA LONGUE VILLE",
        ];

        let patch = Patch::extract(&text, 2, &stop, 0, 31, true, 3);

        let result = vec![
            "M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44166 NANTES SUR LA LONGUE VILLE",
        ];

        assert_eq!(patch.inner_lines, result);

        let text = vec!["M HENRI", "51 RUE BERNARD ROY, 44100 NANTES"];

        let patch = Patch::extract(&text, 1, &stop, 20, 31, true, 3);

        let result = vec!["M HENRI", "51 RUE BERNARD ROY, 44100 NANTES"];

        assert_eq!(patch.inner_lines, result);
    }

    /// Titulaire sans adresse à gauche, domiciliation adressée à droite : le bloc
    /// s'arrête au nom au lieu de glisser dans la colonne de la banque.
    #[test]
    fn going_down_the_block_does_not_slide_into_the_next_column() {
        let stop = Regex::new(r"(?i)(domiciliation|\bIBAN\b)").unwrap();
        let text = vec![
            "     Titulaire du compte                    Domiciliation bancaire",
            " Association des Peintres              Banque Exemple",
            "                                        3 Avenue Hoche",
            "                                        75008 Paris",
        ];

        let patch = Patch::extract(&text, 0, &stop, 5, 23, false, 4);

        assert_eq!(
            patch.lines(),
            vec!["Titulaire du compte", "Association des Peintres"]
        );
    }

    #[test]
    fn test_left_complete() {
        assert_eq!(left_complete("🦀123  6", 0, 0), Some(0));
        assert_eq!(left_complete("0123  678 0🦀  4", 10, 11), Some(6));
    }

    #[test]
    fn test_right_complete() {
        assert_eq!(right_complete("0123  🦀", 6, 6), Some(6));
        assert_eq!(right_complete("0123  🦀78 01  4", 6, 7), Some(11));
    }

    #[test]
    fn test_complete() {
        assert_eq!(
            complete("0  34 🦀78 01  4", 7, 7),
            Some(("34 🦀78 01".to_string(), "0  ".to_string(), 3, 11))
        );
    }
}
