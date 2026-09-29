//! Nettoyage final du bloc titulaire, quel que soit le chemin qui l'a produit.
//!
//! Relevés sur les titulaires rendus pour la campagne de production : un idéogramme
//! lu par l'OCR (« 杁 »), un fragment de mot coupé par le recadrage (« L », « P4 »),
//! des bribes du texte réglementaire de la colonne voisine (« Son utilisation vd »,
//! « statement is inbe »), l'espacement de mise en page (« M.     MONET »), le type de
//! compte imprimé sous le nom (« Compte Cheque »). Rien de tout cela n'est le titulaire,
//! et tout gêne le rapprochement avec un nom connu.

use regex::Regex;

/// Lettres d'un nom ou d'une adresse imprimés en France : l'alphabet latin et ses
/// accents. Le dictionnaire de PP-OCR porte des milliers d'idéogrammes et de symboles,
/// qu'il sort parfois d'un bruit de numérisation.
fn is_latin(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || (('\u{C0}'..='\u{FF}').contains(&c) && c != '×' && c != '÷')
        || matches!(c, 'Œ' | 'œ' | 'Ÿ' | 'Æ' | 'æ')
}

fn keeps(c: char) -> bool {
    is_latin(c) || c.is_whitespace() || c.is_ascii_punctuation() || c == '°' || c == '’'
}

/// Mots du texte réglementaire imprimé à côté du titulaire — « Ce relevé est destiné
/// à… », « This statement is intended for your payees… » —, sans accents, en minuscules.
const BOILERPLATE: [&str; 30] = [
    "utilisation", "garantit", "enregistrement", "operations", "reclamations",
    "imputation", "creanciers", "debiteurs", "quittances", "destine", "remis",
    "vous", "votre", "vos", "evite", "ainsi", "afin", "demande",
    "statement", "intended", "payees", "payors", "debit", "standing", "orders",
    "transfers", "booking", "transactions", "avoiding", "please",
];

fn strip_accents(word: &str) -> String {
    word.chars()
        .map(|c| match c {
            'é' | 'è' | 'ê' | 'ë' => 'e',
            'à' | 'â' | 'ä' => 'a',
            'î' | 'ï' | 'ì' => 'i',
            'ô' | 'ö' => 'o',
            'ù' | 'û' | 'ü' => 'u',
            'ç' => 'c',
            c => c,
        })
        .collect()
}

/// Ligne de ce texte réglementaire : en minuscules pour l'essentiel — un titulaire est
/// presque toujours en capitales, et quand il ne l'est pas, il ne porte pas ces mots —
/// et contenant au moins un de ses mots.
fn is_boilerplate(line: &str) -> bool {
    let letters: Vec<char> = line.chars().filter(|c| c.is_alphabetic()).collect();
    let lower = letters.iter().filter(|c| c.is_lowercase()).count();
    if letters.is_empty() || lower * 2 < letters.len() {
        return false;
    }

    line.split(|c: char| !c.is_alphabetic())
        .map(|w| strip_accents(&w.to_lowercase()))
        .any(|w| BOILERPLATE.contains(&w.as_str()))
}

/// Type de compte imprimé sous le nom : « Compte chèque », « Compte courant »,
/// « Livret A », « CCP ». Pas une qualification du titulaire (« CPTE FONDS DE TIERS »).
fn is_account_type(line: &str) -> bool {
    Regex::new(
        r"(?i)^\s*(compte\s+(ch[eè]ques?|courant|joint|de\s+d[eé]p[oô]t|[eé]pargne|sur\s+livret)|livret\b|ccp\b)",
    )
    .unwrap()
    .is_match(line)
}

/// Nettoie le bloc titulaire ligne à ligne ; `None` s'il n'en reste rien.
pub fn clean_holder(holder: &str) -> Option<String> {
    let lines: Vec<String> = holder
        .lines()
        .map(|line| {
            let kept: String = line.chars().filter(|c| keeps(*c)).collect();
            kept.split_whitespace().collect::<Vec<&str>>().join(" ")
        })
        // un fragment d'un ou deux caractères n'est pas une ligne : « L », « P4 »
        .filter(|line| line.chars().filter(|c| is_latin(*c)).count() > 2)
        .filter(|line| !is_boilerplate(line))
        .filter(|line| !is_account_type(line))
        .collect();

    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stray_ideogram_line_is_dropped() {
        assert_eq!(
            clean_holder("MME KAHLO FRIDA\n杁\n11 RUE DES GRIVES").as_deref(),
            Some("MME KAHLO FRIDA\n11 RUE DES GRIVES")
        );
        // et un symbole collé à une ligne en est retiré
        assert_eq!(
            clean_holder("ɸ\nM MONET CLAUDE").as_deref(),
            Some("M MONET CLAUDE")
        );
    }

    #[test]
    fn fragments_cut_by_the_crop_are_dropped() {
        assert_eq!(
            clean_holder("L\nE.U.R.L. LES PEINTRES 42\nP4\nRUE DES LILAS").as_deref(),
            Some("E.U.R.L. LES PEINTRES 42\nRUE DES LILAS")
        );
    }

    /// Le gabarit bilingue de la Caisse d'Épargne : le recadrage happe les bribes du
    /// texte explicatif de la colonne de droite, en minuscules.
    #[test]
    fn the_neighbouring_regulatory_text_is_dropped() {
        let read = "M DEGAS EDGAR\nSon utilisation vd\nBATIMENT C 17 RUE DES GRIVES\nvous évite ainsì\nstatement is inbe\ndebit, Standing\n75009 PARIS";
        assert_eq!(
            clean_holder(read).as_deref(),
            Some("M DEGAS EDGAR\nBATIMENT C 17 RUE DES GRIVES\n75009 PARIS")
        );
        // un titulaire en minuscules ne porte pas ces mots : il reste
        assert_eq!(
            clean_holder("Mlle Frida Kahlo\n117 rue des bourdonnieres\n44200 Nantes").as_deref(),
            Some("Mlle Frida Kahlo\n117 rue des bourdonnieres\n44200 Nantes")
        );
        // une association en capitales non plus, même avec « POUR »
        assert_eq!(
            clean_holder("ASSOC. AMIS POUR LA PEINTURE").as_deref(),
            Some("ASSOC. AMIS POUR LA PEINTURE")
        );
    }

    #[test]
    fn layout_spacing_is_collapsed() {
        assert_eq!(
            clean_holder("M.     MONET CLAUDE\n4 IMPASSE DE LA CATHEDRALE").as_deref(),
            Some("M. MONET CLAUDE\n4 IMPASSE DE LA CATHEDRALE")
        );
    }

    #[test]
    fn the_account_type_is_not_a_holder_line() {
        assert_eq!(
            clean_holder("M MONET CLAUDE\nCompte Cheque").as_deref(),
            Some("M MONET CLAUDE")
        );
        // une qualification du compte, elle, reste
        assert_eq!(
            clean_holder("FEDERATION DES PEINTRES\nCPTE FONDS DE TIERS").as_deref(),
            Some("FEDERATION DES PEINTRES\nCPTE FONDS DE TIERS")
        );
    }

    #[test]
    fn nothing_left_gives_nothing() {
        assert_eq!(clean_holder("杁\nL"), None);
    }
}
