use regex::Regex;

use super::patch::{right_complete, Patch};

/// Un titulaire tient rarement en plus de six lignes : libellé, deux noms, complément,
/// voie, ville. Au-delà, on est sorti du bloc.
const MAX_HOLDER_LINES: usize = 6;

/// `nb_line` borne le bloc quand rien d'autre ne l'arrête. Le bloc s'étend en dessous du
/// libellé jusqu'à une frontière — ligne vide, mot-clé, IBAN ou BIC — ou jusqu'à cette
/// borne, la première atteinte l'emportant.
///
/// Un plafond fixe de trois lignes coupait un couple suivi de son adresse dès que le code
/// postal n'était pas reconnu : le libellé comptant pour une ligne, il ne restait que le
/// nom.
pub fn find_simple_account_holder(text: &str, nb_line: usize) -> Option<String> {
    let account_holder = Regex::new(r"(?i)(titulaire)").unwrap();
    // l'IBAN et le BIC ferment le bloc : ils suivent souvent le titulaire directement
    let stop = Regex::new(
        r"(?i)(domiciliation|cadre réservé|identification|\bIBAN\b|\bBIC\b|FR\d{2} ?\d{4})",
    )
    .unwrap();
    let lines: Vec<&str> = text.lines().collect();

    let result = if let Some((index, m)) = lines.iter().enumerate().find_map(|(i, line)| {
        if account_holder.is_match(line) {
            Some((i, account_holder.find(line).unwrap()))
        } else {
            None
        }
    }) {
        let start = m.start();
        let end = m.end();

        // le bloc court jusqu'à la première ligne vide sous le libellé, ou jusqu'à la
        // borne haute — plutôt que jusqu'à un compte fixe
        let block_end = lines
            .iter()
            .enumerate()
            .skip(index + 1)
            .find(|(_, line)| line.trim().is_empty())
            .map(|(i, _)| i)
            .unwrap_or(lines.len());
        let span = (block_end - index).clamp(nb_line, MAX_HOLDER_LINES);

        let patch = Patch::extract(&lines, index, &stop, start, end - 1, false, span);
        clean(patch.lines())
    } else {
        None
    };

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

fn clean(lines: Vec<String>) -> Option<Vec<String>> {
    let account_holder = Regex::new(r"(?i)(titulaire)").unwrap();
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
        r"(?i)^\s*(titulaire|intitul[eé])(\s+du|\s+de)?(\s+compte)?\s*([:\-/]\s*)?(\(?account\s+(owner|holder)\)?)?\s*[:\-]?\s*",
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

    /// Le bloc ne dépasse jamais six lignes, quoi qu'il arrive.
    #[test]
    fn the_block_is_capped() {
        let text = "Titulaire\nL1\nL2\nL3\nL4\nL5\nL6\nL7\nL8";
        let found = find_simple_account_holder(text, 3).unwrap();

        assert!(found.lines().count() <= MAX_HOLDER_LINES);
    }
}
