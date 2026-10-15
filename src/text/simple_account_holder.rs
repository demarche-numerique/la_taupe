use regex::Regex;

use super::address::is_english_label;
use super::patch::{right_complete, Patch};

/// Un titulaire tient rarement en plus de six lignes : libellé, deux noms, complément,
/// voie, ville. Au-delà, on est sorti du bloc.
const MAX_HOLDER_LINES: usize = 6;

/// Lignes blanches, ou traduction du libellé, tolérées entre le libellé et le titulaire
/// dans un texte mis en page.
const MAX_LEADING_LINES: usize = 3;

/// `nb_line` borne le bloc quand rien d'autre ne l'arrête. Le bloc s'étend en dessous du
/// libellé jusqu'à une frontière — ligne vide, mot-clé, IBAN ou BIC — ou jusqu'à cette
/// borne, la première atteinte l'emportant.
///
/// Un plafond fixe de trois lignes coupait un couple suivi de son adresse dès que le code
/// postal n'était pas reconnu : le libellé comptant pour une ligne, il ne restait que le
/// nom.
pub fn find_simple_account_holder(text: &str, nb_line: usize) -> Option<String> {
    find_holder(text, nb_line, false)
}

/// Le même repli sur le texte d'un PDF mis en page (`pdftotext -layout`), dont les
/// colonnes sont conservées. Trois mises en page s'y ajoutent, qui n'ont de sens qu'avec
/// elles :
///
/// - « Bénéficiaire » et « Intitulé du compte » désignent le titulaire comme « titulaire »,
///   pourvu qu'ils tiennent seuls dans leur cellule : dans une phrase de la notice, ils ne
///   désignent rien ;
/// - le titulaire peut être séparé de son libellé par des lignes blanches, ou par la
///   traduction du libellé (« Account Owner ») : la première ligne blanche ne ferme le
///   bloc qu'une fois le titulaire commencé ;
/// - le titulaire peut être à droite de son libellé, sur la même ligne, au-delà d'un
///   blanc de colonne (« TITULAIRE      TRESORERIE DE … »).
pub fn find_laid_out_account_holder(text: &str) -> Option<String> {
    find_holder(text, 3, true)
}

fn find_holder(text: &str, nb_line: usize, laid_out: bool) -> Option<String> {
    // l'IBAN et le BIC ferment le bloc : ils suivent souvent le titulaire directement
    let stop = Regex::new(
        r"(?i)(domiciliation|cadre réservé|identification|\bIBAN\b|\bBIC\b|FR\d{2} ?\d{4})",
    )
    .unwrap();
    let lines: Vec<&str> = text.lines().collect();

    // Hors mise en page, seul le premier libellé est examiné.
    let labels = labels(&lines, laid_out);
    let labels = if laid_out {
        &labels[..]
    } else {
        &labels[..labels.len().min(1)]
    };

    let result = labels.iter().find_map(|&(index, start, end)| {
        under_the_label(&lines, index, start, end, nb_line, laid_out, &stop).or_else(|| {
            laid_out
                .then(|| right_of_the_label(&lines, index, start, end, &stop))
                .flatten()
        })
    });

    if result.is_some() {
        return result.map(|r| r.join("\n"));
    }

    // last chance, try for one line by civilite
    let civilite =
        Regex::new(r"(?i)(^|\s)(m|monsieur|mr|mademoiselle|ml|mle|mlle|melle|madame|mme)\.?\s")
            .unwrap();

    if let Some(index) = lines.iter().position(|x| civilite.is_match(x)) {
        let m = civilite.find(lines[index]).unwrap();
        let start = m.start();

        return right_complete(lines[index], start, m.end() - 1)
            .map(|end| lines[index][start..=end].trim().to_string());
    }

    None
}

/// Libellés du titulaire, dans l'ordre du texte : la ligne, puis le premier et le
/// dernier caractère du libellé.
fn labels(lines: &[&str], laid_out: bool) -> Vec<(usize, usize, usize)> {
    let titulaire = Regex::new(r"(?i)titulaire").unwrap();
    // seul dans sa cellule : en début de ligne ou après un blanc de colonne, et suivi
    // d'un deux-points, d'un blanc de colonne ou de la fin de ligne
    let cell = Regex::new(
        r"(?i)(?:^|\s\s)\s*(b[ée]n[ée]ficiaire|intitul[ée]\s+du\s+compte)\s*(?::|\s\s|$)",
    )
    .unwrap();

    lines
        .iter()
        .enumerate()
        .filter_map(|(i, line)| {
            let m = titulaire.find(line).or_else(|| {
                laid_out
                    .then(|| cell.captures(line).and_then(|c| c.get(1)))
                    .flatten()
            })?;
            let start = line[..m.start()].chars().count();
            Some((i, start, start + m.as_str().chars().count() - 1))
        })
        .collect()
}

/// Le titulaire sous son libellé, dans la colonne du libellé.
fn under_the_label(
    lines: &[&str],
    index: usize,
    start: usize,
    end: usize,
    nb_line: usize,
    laid_out: bool,
    stop: &Regex,
) -> Option<Vec<String>> {
    // première ligne du titulaire : celle qui suit le libellé ou, mis en page, la
    // première qui ne soit ni blanche ni la traduction du libellé
    let first = laid_out
        .then(|| {
            (index + 1..lines.len())
                .take(MAX_LEADING_LINES + 1)
                .find(|&i| !lines[i].trim().is_empty() && !is_english_label(lines[i]))
        })
        .flatten()
        .unwrap_or(index + 1);
    let leading = first - index - 1;

    // le bloc court jusqu'à la première ligne vide sous le libellé, ou jusqu'à la
    // borne haute — plutôt que jusqu'à un compte fixe
    let block_end = (first..lines.len())
        .find(|&i| lines[i].trim().is_empty())
        .unwrap_or(lines.len());
    let span = (block_end - index).clamp(nb_line + leading, MAX_HOLDER_LINES + leading);

    // les lignes sautées sont blanchies : la traduction du libellé ferait commencer le
    // titulaire, et la ligne blanche qui la suit le fermerait
    let lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| if i > index && i < first { "" } else { line })
        .collect();

    let patch = Patch::extract(&lines, index, stop, start, end, false, span);
    clean(patch.lines())
}

/// Le titulaire à droite de son libellé, au-delà d'un blanc de colonne. Il se poursuit
/// sur les lignes suivantes tant que la colonne du libellé y reste blanche : un libellé
/// en dessous (« DOMICILIATION ») ouvre un autre champ.
fn right_of_the_label(
    lines: &[&str],
    index: usize,
    start: usize,
    end: usize,
    stop: &Regex,
) -> Option<Vec<String>> {
    let chars: Vec<char> = lines[index].chars().collect();
    // la cellule du libellé, sa suite comprise (« du compte : »)
    let label_end = right_complete(lines[index], start, end)?;
    let value = (label_end + 1..chars.len()).find(|&i| !chars[i].is_whitespace())?;

    let mut found = vec![];
    for (offset, line) in lines[index..].iter().take(MAX_HOLDER_LINES).enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if offset > 0
            && chars
                .iter()
                .take(value)
                .skip(start)
                .any(|c| !c.is_whitespace())
        {
            break;
        }
        // la suite de la valeur commence dans sa colonne, pas dans une colonne voisine
        let Some(from) = (value.min(chars.len())..chars.len().min(value + 3))
            .find(|&i| !chars[i].is_whitespace())
        else {
            break;
        };
        let to = right_complete(line, from, from)?;
        let cell: String = chars[from..=to].iter().collect();
        if stop.is_match(&cell) {
            break;
        }
        found.push(cell);
    }

    clean(found)
}

fn clean(lines: Vec<String>) -> Option<Vec<String>> {
    let account_holder =
        Regex::new(r"(?i)(titulaire|b[ée]n[ée]ficiaire|intitul[ée]\s+du\s+compte)").unwrap();
    // Les frontières qui ont arrêté le bloc en font partie : on les retire ici, IBAN et
    // BIC compris — ils suivent souvent le titulaire sans ligne vide.
    let headers = Regex::new(
        r"(?i)(titulaire|domiciliation|cadre réservé|identification|numero de|\bIBAN\b|\bBIC\b|FR\d{2} ?\d{4})",
    )
    .unwrap();

    // Le libellé peut être en tête de la ligne du nom sans deux-points pour les
    // séparer : « TITULAIRE DU MME PRENOM NOM ». Jeter la ligne entière parce qu'elle
    // porte le libellé, c'est jeter le titulaire avec. On retire le libellé, on garde
    // le reste.
    // Le libellé peut être suivi de sa traduction : « Titulaire du compte - Account
    // Owner ». Elle est retirée avec lui.
    let label_prefix = Regex::new(
        r"(?i)^\s*(titulaire|intitul[eé]|b[ée]n[ée]ficiaire)(\s+du|\s+de)?(\s+compte)?\s*([:\-/]\s*)?(\(?account\s+(owner|holder)\)?)?\s*[:\-]?\s*",
    )
    .unwrap();

    let vec: Vec<String> = lines
        .into_iter()
        .map(|line| {
            if account_holder.is_match(&line) && line.contains(':') {
                line.split(':').nth(1).unwrap().to_string()
            } else if account_holder.is_match(&line) {
                label_prefix.replace(&line, "").to_string()
            } else {
                line
            }
        })
        .filter(|line| !headers.is_match(line))
        .filter(|line| !is_english_label(line))
        .filter(|line| !crate::ocr::is_label_or_noise(line))
        .filter(|line| !line.is_empty())
        .map(|line| {
            if line.contains(':') {
                line.split(':').nth(1).unwrap().to_string()
            } else {
                line
            }
        })
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();

    if vec.is_empty() {
        None
    } else {
        Some(vec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sans code postal reconnu, le repli tronquait le bloc à deux lignes de contenu :
    /// le libellé comptait pour une, et le plafond était de trois.
    #[test]
    fn a_couple_with_address_is_kept_whole_without_postal_code() {
        let text = "Titulaire du compte\nM MATISSE HENRI\nOU MME KAHLO FRIDA\n51 RUE BERNARD ROY\nNANTES CEDEX 1\nIBAN FR76 3000 1000 6449 1900 9562 088";

        assert_eq!(
            find_simple_account_holder(text, 3).as_deref(),
            Some("M MATISSE HENRI\nOU MME KAHLO FRIDA\n51 RUE BERNARD ROY\nNANTES CEDEX 1")
        );
    }

    /// L'IBAN et le BIC ferment le bloc et n'en font pas partie.
    #[test]
    fn iban_and_bic_close_the_block() {
        let text = "Titulaire\nM MATISSE HENRI\nBP 1234\nNANTES\nIBAN FR76 3000 1000 6449 1900 9562 088\nBIC SOGEFRPP";

        assert_eq!(
            find_simple_account_holder(text, 3).as_deref(),
            Some("M MATISSE HENRI\nBP 1234\nNANTES")
        );
    }

    /// Une ligne vide ferme le bloc.
    #[test]
    fn a_blank_line_closes_the_block() {
        let text =
            "Titulaire\nM MATISSE HENRI\n51 RUE BERNARD ROY\n\nDomiciliation\nAGENCE DE NANTES";

        assert_eq!(
            find_simple_account_holder(text, 3).as_deref(),
            Some("M MATISSE HENRI\n51 RUE BERNARD ROY")
        );
    }

    /// Le libellé peut précéder le nom sur la même ligne, sans deux-points ; et se
    /// poursuivre sur la ligne suivante (« TITULAIRE DU » / « COMPTE : »). Le titulaire
    /// commence alors à droite du libellé, sur les deux lignes.
    #[test]
    fn a_label_split_over_two_lines_keeps_both_holder_lines() {
        let text = "RELEVE D IDENTITE BANCAIRE\n\nTITULAIRE DU MME PRENOM NOM OU M\nCOMPTE :     PRENOM2 NOM2\n\nIBAN FR76";

        assert_eq!(
            find_simple_account_holder(text, 3).as_deref(),
            Some("MME PRENOM NOM OU M\nPRENOM2 NOM2")
        );
    }

    /// Mis en page, le titulaire peut être à droite de son libellé, au-delà d'un blanc de
    /// colonne ; le libellé du dessous ouvre un autre champ.
    #[test]
    fn a_holder_to_the_right_of_its_label_is_found() {
        let text = "\
Releve d identite bancaire
TITULAIRE                     TRESORERIE DE GIVERNY
DOMICILIATION                 BDF PARIS";

        assert_eq!(
            find_laid_out_account_holder(text).as_deref(),
            Some("TRESORERIE DE GIVERNY")
        );

        let text = "\
TITULAIRE :    SERVICE DE GESTION COMPTABLE D'ARGENTEUIL

DOMICILIATION : BDF";

        assert_eq!(
            find_laid_out_account_holder(text).as_deref(),
            Some("SERVICE DE GESTION COMPTABLE D'ARGENTEUIL")
        );
        // hors mise en page, rien ne change
        assert_eq!(find_simple_account_holder(text, 3), None);
    }

    /// La valeur se poursuit sous elle tant que la colonne du libellé reste blanche.
    #[test]
    fn a_holder_to_the_right_continues_below_itself() {
        let text = "\
TITULAIRE         FONDATION CLAUDE MONET
                  ET BERTHE MORISOT
ADRESSE           12 AVENUE DES IRIS";

        assert_eq!(
            find_laid_out_account_holder(text).as_deref(),
            Some("FONDATION CLAUDE MONET\nET BERTHE MORISOT")
        );
    }

    /// Un autre libellé à droite du libellé n'est pas le titulaire.
    #[test]
    fn a_neighbouring_label_is_not_a_holder() {
        let text = "Titulaire du compte                 Domiciliation\nIBAN FR76 3000 1000 6449 1900 9562 088";

        assert_eq!(find_laid_out_account_holder(text), None);
    }

    /// « Bénéficiaire » seul sur sa ligne, le titulaire une ligne blanche plus bas.
    #[test]
    fn a_holder_below_a_beneficiary_label_and_a_blank_line_is_found() {
        let text = "\
                       RELEVÉ D'IDENTITÉ BANCAIRE


BÉNÉFICIAIRE

ATELIER MONET-RENOIR (ASSOCIATION DES PEINTRES)



RIB";

        assert_eq!(
            find_laid_out_account_holder(text).as_deref(),
            Some("ATELIER MONET-RENOIR (ASSOCIATION DES PEINTRES)")
        );
        assert_eq!(find_simple_account_holder(text, 3), None);
    }

    /// Dans une phrase de la notice, « bénéficiaire » ne désigne rien.
    #[test]
    fn a_beneficiary_in_a_sentence_is_not_a_label() {
        let text = "Remettez ce relevé au bénéficiaire du virement.\nVous recevrez un avis.";

        assert_eq!(find_laid_out_account_holder(text), None);
    }

    /// Le libellé, sa traduction, deux lignes blanches, puis le titulaire et son adresse.
    #[test]
    fn the_translated_label_and_blank_lines_do_not_close_the_block() {
        let text = "\
TITULAIRE DU COMPTE
 Account Owner


 PAUL CEZANNE
 12 RUE DES PEINTRES
 13100 AIX EN PROVENCE

IBAN FR76 3000 1000 6449 1900 9562 088";

        assert_eq!(
            find_laid_out_account_holder(text).as_deref(),
            Some("PAUL CEZANNE\n12 RUE DES PEINTRES\n13100 AIX EN PROVENCE")
        );
    }

    /// Le bloc ne dépasse jamais six lignes, quoi qu'il arrive.
    #[test]
    fn the_block_is_capped() {
        let text = "Titulaire\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8";
        let found = find_simple_account_holder(text, 3).unwrap();

        assert!(found.lines().count() <= MAX_HOLDER_LINES);
    }
}
