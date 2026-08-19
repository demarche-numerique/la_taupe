use iban::Iban;
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::{
    fi_extract::IbanToBankName,
    text::{address::find_account_holder_addr, simple_account_holder::find_simple_account_holder},
};

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct Rib {
    account_holder: Option<String>,
    iban: String,
    bic: Option<String>,
    bank_name: Option<String>,
}

impl Rib {
    pub fn from_iban(iban: String, account_holder: Option<String>, bic: Option<String>) -> Self {
        let bank_name = IbanToBankName::new().bank_name(&iban);

        Rib {
            account_holder,
            iban,
            bic,
            bank_name,
        }
    }
    pub fn parse(text: String) -> Option<Self> {
        let account_holder = find_account_holder_addr(&text)
            .map(|addr| addr.lines().join("\n"))
            .or_else(|| find_simple_account_holder(&text, 3));

        let iban = extract_iban(&text)?;
        let bic = extract_fr_bic(&text, Some(&iban));

        Some(Rib::from_iban(iban, account_holder, bic))
    }

    pub fn iban(&self) -> &str {
        &self.iban
    }

    pub fn bic(&self) -> Option<&str> {
        self.bic.as_deref()
    }

    pub fn account_holder(&self) -> Option<&str> {
        self.account_holder.as_deref()
    }

    pub fn bank_name(&self) -> Option<&str> {
        self.bank_name.as_deref()
    }
}

/// Normalise un IBAN : séparateurs retirés, majuscules.
pub fn normalize_iban(iban: &str) -> String {
    iban.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Valeur d'un caractère de numéro de compte pour le calcul de la clé RIB.
/// A,J→1 · B,K,S→2 · C,L,T→3 · D,M,U→4 · E,N,V→5 · F,O,W→6 · G,P,X→7 · H,Q,Y→8 · I,R,Z→9
/// Noter que la troisième série démarre à 2 : aucune lettre de S à Z ne vaut 1.
fn account_char_value(c: char) -> Option<u64> {
    match c {
        '0'..='9' => Some(c as u64 - '0' as u64),
        'A'..='I' => Some(c as u64 - 'A' as u64 + 1),
        'J'..='R' => Some(c as u64 - 'J' as u64 + 1),
        'S'..='Z' => Some(c as u64 - 'S' as u64 + 2),
        _ => None,
    }
}

/// Clé RIB française : 97 - (89·banque + 15·guichet + 3·compte) mod 97.
pub fn rib_key(bank: &str, branch: &str, account: &str) -> Option<u8> {
    fn to_number(s: &str) -> Option<u64> {
        s.chars().try_fold(0u64, |acc, c| {
            account_char_value(c).map(|v| (acc * 10 + v) % 97)
        })
    }

    let rest = (89 * to_number(bank)? + 15 * to_number(branch)? + 3 * to_number(account)?) % 97;

    Some((97 - rest) as u8)
}

/// Découpe un IBAN français en (banque, guichet, compte, clé RIB).
fn split_fr_iban(iban: &str) -> Option<(String, String, String, String)> {
    let normalized = normalize_iban(iban);

    if normalized.len() != 27 || !normalized.starts_with("FR") {
        return None;
    }

    let c: Vec<char> = normalized.chars().collect();

    Some((
        c[4..9].iter().collect(),
        c[9..14].iter().collect(),
        c[14..25].iter().collect(),
        c[25..27].iter().collect(),
    ))
}

/// Vérifie la clé RIB (positions 25-26) d'un IBAN français.
///
/// Contrôle indépendant du mod-97 de l'IBAN : les deux combinés font tomber le taux de
/// faux positifs de 1/97 à ~1/9400 quand on teste des candidats issus de l'OCR.
pub fn check_rib_key(iban: &str) -> bool {
    let Some((bank, branch, account, key)) = split_fr_iban(iban) else {
        return false;
    };

    match (rib_key(&bank, &branch, &account), key.parse::<u8>()) {
        (Some(expected), Ok(found)) => expected == found,
        _ => false,
    }
}

/// Reconstruit un IBAN FR à partir d'un BBAN de 23 caractères, en calculant la clé de
/// contrôle IBAN. Permet de retrouver l'IBAN depuis le seul tableau RIB.
pub fn iban_from_bban(bban: &str) -> Option<String> {
    let bban = normalize_iban(bban);

    if bban.len() != 23 {
        return None;
    }

    // "FR00" déplacé en fin de chaîne, lettres converties (A=10 … Z=35).
    let rest = bban
        .chars()
        .chain("FR00".chars())
        .try_fold(0u64, |acc, c| match c {
            '0'..='9' => Some((acc * 10 + (c as u64 - '0' as u64)) % 97),
            'A'..='Z' => Some((acc * 100 + (c as u64 - 'A' as u64 + 10)) % 97),
            _ => None,
        })?;

    Some(format!("FR{:02}{}", 98 - rest, bban))
}

pub fn replace_char_by_digit_in_2_and_3_position(ibans: Vec<String>) -> Vec<String> {
    ibans
        .into_iter()
        .map(|x| {
            let mut chars: Vec<char> = x.chars().collect();
            if chars.len() > 2 && (chars[2] == 'O' || chars[2] == 'o') {
                chars[2] = '0';
            }
            if chars.len() > 3 && (chars[3] == 'O' || chars[3] == 'o') {
                chars[3] = '0';
            }
            chars.into_iter().collect()
        })
        .collect()
}

pub fn extract_iban(text: &str) -> Option<String> {
    let french_iban_re = Regex::new(r"(?<iban>FR[[[:digit:]]O]{2}([[[:space:]]\|,]*[[:alnum:]]{4}){5})([[[:space:]]|,]*[[:alnum:]][[:digit:]]{2})").unwrap();

    let to_remove = Regex::new(r"[[[:space:]]|,]*").unwrap();

    let mut ibans = french_iban_re
        .find_iter(text)
        .map(|x| x.as_str().to_string())
        // change multiple spaces by one space
        .map(|x| to_remove.replace_all(&x, "").to_string())
        .collect::<Vec<String>>();

    ibans = replace_char_by_digit_in_2_and_3_position(ibans);

    let found_ibans = ibans
        .clone()
        .into_iter()
        .filter_map(|x| x.parse::<Iban>().ok())
        .collect::<Vec<Iban>>();

    if !found_ibans.is_empty() {
        return Some(found_ibans[0].to_string());
    }

    // sometimes the iban is written with weird space (credit_agricole_2.txt)
    // so we try to match by removing all spaces
    let text_without_spaces = text.replace(" ", "");
    ibans = french_iban_re
        .find_iter(&text_without_spaces)
        .map(|x| x.as_str().to_string())
        .map(|x| to_remove.replace_all(&x, "").to_string())
        .collect::<Vec<String>>();

    ibans = replace_char_by_digit_in_2_and_3_position(ibans);

    let found_ibans = ibans
        .clone()
        .into_iter()
        .filter_map(|x| x.parse::<Iban>().ok())
        .collect::<Vec<Iban>>();

    if !found_ibans.is_empty() {
        return Some(found_ibans[0].to_string());
    }

    let lax_frenc_iban_re = Regex::new(r"(?<iban>FR[[:alnum:]]{2}([[[:space:]]\|]*[[:alnum:]]{4}){5})([[[:space:]]|]*[[:alnum:]][[:digit:]]{2})").unwrap();

    let lax_ibans = lax_frenc_iban_re
        .find_iter(text)
        .map(|x| x.as_str().to_string())
        // change multiple spaces by one space
        .map(|x| to_remove.replace_all(&x, "").to_string())
        .collect::<Vec<String>>();

    if lax_ibans.len() < 2 {
        return None;
    }

    // we take the 2 first iban and count the number of different characters
    let iban1 = lax_ibans[0].clone();
    let iban2 = lax_ibans[1].clone();

    let mut differences = Vec::new();

    // Itérer sur les caractères des chaînes
    for (index, (c1, c2)) in iban1.chars().zip(iban2.chars()).enumerate() {
        if c1 != c2 {
            differences.push(index);
        }
    }

    // to many combinations
    if differences.len() > 10 {
        return None;
    }

    let mut combinations = Vec::new();
    let num_differences = differences.len();
    let num_combinations = 1 << num_differences; // 2^n

    for i in 0..num_combinations {
        let mut combo = iban1.to_string();
        for (j, &diff_pos) in differences.iter().enumerate() {
            if (i >> j) & 1 == 1 {
                let c = iban2.chars().nth(diff_pos).unwrap();
                combo.replace_range(diff_pos..diff_pos + 1, &c.to_string());
            }
        }
        combinations.push(combo);
    }

    let found_ibans = combinations
        .into_iter()
        .filter_map(|x| x.parse::<Iban>().ok())
        .collect::<Vec<Iban>>();

    if found_ibans.len() == 1 {
        Some(found_ibans[0].to_string())
    } else {
        None
    }
}

/// Écarte les candidats qui ne sont qu'un fragment de l'IBAN voisin.
///
/// Le motif `[A-Z]{4}FR[A-Z0-9]{2}` capture quatre lettres quelconques suivies du début
/// de l'IBAN : « NBICFR67190 » sur un IBAN commençant par FR67190. Le libellé collé à sa
/// valeur suffit à le produire, et la passe sans espaces le provoque systématiquement.
///
/// Le candidat était alors soit retourné tel quel quand le vrai BIC avait été mal lu,
/// soit rejeté avec lui pour cause d'ambiguïté. Ce qui suit le code établissement doit
/// être un code pays, pas la suite d'un numéro de compte.
fn is_iban_fragment(candidate: &str, iban: &str) -> bool {
    let candidate = normalize_iban(candidate);

    candidate.len() > 4 && normalize_iban(iban).starts_with(&candidate[4..])
}

/// Recolle un BIC imprimé dans un tableau à une lettre par cellule.
///
/// L'OCR rend les cellules comme des mots courts, partiellement fusionnés — « Ps sƫ FRP
/// P NƫE » — et le motif du BIC ne voit jamais la suite entière. Sur chaque ligne, une
/// série de mots d'un à trois caractères alphanumériques dont le total atteint huit
/// lettres est refermée en un mot ; le reste est conservé tel quel. Le trait vertical
/// d'une cellule collé à un T donne « ƫ » : normalisé.
pub fn join_cell_letters(text: &str) -> String {
    let text = text.replace(['ƫ', 'Ƭ', 'ŧ', 'Ŧ'], "T");

    text.lines()
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            let mut out: Vec<String> = Vec::new();
            let mut i = 0;
            while i < words.len() {
                // un mot court, mais pas le libellé « BIC » ni « IBAN » eux-mêmes
                let is_cell = |w: &str| {
                    let n = w.chars().count();
                    (1..=3).contains(&n)
                        && w.chars().all(|c| c.is_ascii_alphanumeric())
                        && !w.eq_ignore_ascii_case("BIC")
                };
                let run_end = (i..words.len())
                    .take_while(|&j| is_cell(words[j]))
                    .last()
                    .map(|j| j + 1)
                    .unwrap_or(i);
                let total: usize = words[i..run_end].iter().map(|w| w.chars().count()).sum();
                if run_end - i >= 3 && total >= 8 {
                    out.push(words[i..run_end].concat().to_uppercase());
                    i = run_end;
                } else {
                    out.push(words[i].to_string());
                    i += 1;
                }
            }
            out.join(" ")
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// `iban`, lorsqu'il est connu, sert deux fois : à écarter les fragments d'IBAN que le
/// motif prend pour un BIC, et — via le registre BCE — à n'accepter qu'un BIC dont le
/// code établissement concorde avec le code banque. Le BIC n'a aucune redondance
/// interne ; ce recoupement est la seule validation possible, et il rejette un candidat
/// lu à une lettre près (« PSSTFRPPNTH ») comme un candidat fabriqué de toutes pièces.
/// Quand le registre ne connaît pas l'établissement, on ne filtre pas.
pub fn extract_fr_bic(content: &str, iban: Option<&str>) -> Option<String> {
    let fr_without_space = Regex::new(r"[A-Z]{4}FR[A-Z0-9]{2}([A-Z0-9]{3})?").unwrap();
    let fr_with_xxx_with_space = Regex::new(r"[A-Z]{4}\s?FR\s?[A-Z0-9]{2}\s?XXX?").unwrap();

    let expected_prefix = iban.and_then(|iban| IbanToBankName::new().expected_bic_prefix(iban));

    let get_unique_matches = |regex: &Regex, text: &str| -> Vec<String> {
        regex
            .find_iter(text)
            .map(|m| m.as_str().to_string())
            .filter(|candidate| !iban.is_some_and(|iban| is_iban_fragment(candidate, iban)))
            .filter(|candidate| {
                expected_prefix
                    .as_ref()
                    .is_none_or(|prefix| normalize_iban(candidate).starts_with(prefix.as_str()))
            })
            .unique()
            .collect()
    };

    let mut fr_without_space_matches = get_unique_matches(&fr_without_space, content);
    log::trace!("fr_without_space_matches: {:?}", fr_without_space_matches);
    if fr_without_space_matches.len() == 1 {
        return Some(fr_without_space_matches.pop().unwrap());
    }

    let mut fr_with_xxx_with_space_matches = get_unique_matches(&fr_with_xxx_with_space, content);
    log::trace!(
        "fr_with_xxx_with_space_matches: {:?}",
        fr_with_xxx_with_space_matches
    );
    if fr_with_xxx_with_space_matches.len() == 1 {
        return Some(fr_with_xxx_with_space_matches.pop().unwrap());
    }

    // remove all spaces and try again
    let whitespace_regex = Regex::new(r"\s+").unwrap();
    let content_without_spaces = whitespace_regex.replace_all(content, "");
    let mut joined_fr_without_space_matches =
        get_unique_matches(&fr_without_space, &content_without_spaces);
    log::trace!(
        "joined_fr_without_space_matches: {:?}",
        joined_fr_without_space_matches
    );
    if joined_fr_without_space_matches.len() == 1 {
        return Some(joined_fr_without_space_matches.pop().unwrap());
    }

    // try known banks BICs
    let caisse_epargne_bic = Regex::new(r"CEPAFRPP[A-Z0-9]{3}").unwrap();
    let mut caisse_epargne_bic_matches =
        get_unique_matches(&caisse_epargne_bic, &content_without_spaces);
    if caisse_epargne_bic_matches.len() == 1 {
        return Some(caisse_epargne_bic_matches.pop().unwrap());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_iban() {
        let iban = "FR76 3000 1000 6449 1900 9562 088";
        assert_eq!(extract_iban(iban).unwrap(), iban);

        let other_iban = "FR76 | 3000

          1000 | 6449

          1900 | 9562 | 088";
        assert_eq!(extract_iban(other_iban).unwrap(), iban);

        let iban_with_faults = "
          FRTS 3000 1000 6449 1900 9562 088
          FR76 3000 BOO0 6666 1900 9562 088
        ";

        assert_eq!(extract_iban(iban_with_faults).unwrap(), iban);
    }

    #[test]
    fn test_rib_key() {
        // IBAN de démonstration de la Banque de France, découpé en ses composants.
        assert_eq!(rib_key("30001", "00064", "49190095620"), Some(88));

        // les lettres du numéro de compte passent par la table de conversion
        assert_eq!(rib_key("30001", "00064", "4919009562A"), Some(85));

        // un caractère hors alphanumérique invalide le calcul
        assert_eq!(rib_key("30001", "00064", "4919009562-"), None);
    }

    #[test]
    fn test_check_rib_key() {
        assert!(check_rib_key(IBAN));
        assert!(check_rib_key("FR7630001000644919009562088"));

        // clé RIB altérée
        assert!(!check_rib_key("FR7630001000644919009562087"));

        // IBAN non français, ou de longueur inattendue
        assert!(!check_rib_key("DE89370400440532013000"));
        assert!(!check_rib_key("FR76 3000"));
    }

    #[test]
    fn test_iban_from_bban() {
        // le tableau RIB seul suffit à reconstituer l'IBAN
        assert_eq!(
            iban_from_bban("30001000644919009562088").as_deref(),
            Some("FR7630001000644919009562088")
        );

        assert_eq!(iban_from_bban("trop court"), None);
    }

    /// Le motif du BIC attrape quatre lettres quelconques suivies du début de l'IBAN.
    /// Connaître l'IBAN suffit à les distinguer d'un vrai code établissement.
    #[test]
    fn iban_fragments_are_not_mistaken_for_a_bic() {
        let iban = "FR6719069478526623075402Z93";

        // observés sur corpus : le libellé collé à sa valeur produit ces candidats
        assert!(is_iban_fragment("NBICFR67190", iban));
        assert!(is_iban_fragment("DGARFR67190", iban));

        // un vrai BIC ne prolonge pas l'IBAN
        assert!(!is_iban_fragment("PSSTFRPPNTE", iban));
        assert!(!is_iban_fragment("CMCIFR2A", iban));
        assert!(!is_iban_fragment("SOGEFRPP", iban));
    }

    #[test]
    fn bic_is_extracted_despite_a_glued_label() {
        let iban = "FR6719069478526623075402Z93";

        // le vrai BIC coexiste avec le faux candidat : renoncer serait perdre les deux
        // (19069 est KBLX au registre)
        let text = "IBANFR6719069478526623075402Z93 BIC KBLXFRPPXXX";
        assert_eq!(
            extract_fr_bic(text, Some(iban)).as_deref(),
            Some("KBLXFRPPXXX")
        );

        // le vrai BIC est illisible : mieux vaut ne rien rendre qu'un fragment d'IBAN
        let text = "IBANFR6719069478526623075402Z93 BIC KB1XFRPP";
        assert_eq!(extract_fr_bic(text, Some(iban)), None);
    }

    /// Un BIC en tableau, une lettre par cellule, doit être recollé — et seulement lui :
    /// les mots ordinaires de la ligne restent séparés.
    #[test]
    fn cell_letters_are_joined_into_a_bic() {
        assert_eq!(
            join_cell_letters("BIC P S S T F R P P N T E"),
            "BIC PSSTFRPPNTE"
        );
        assert_eq!(
            join_cell_letters("C E P A F R P P 4 4 4 Code"),
            "CEPAFRPP444 Code"
        );
        // cellules partiellement fusionnées, trait collé au T
        assert_eq!(join_cell_letters("Ps sƫ FRP P NƫE"), "PSSTFRPPNTE");
        // trop court pour être un BIC : on ne recolle pas
        assert_eq!(join_cell_letters("M OU MME X"), "M OU MME X");
        // et le motif s'y applique ensuite
        assert_eq!(
            extract_fr_bic(&join_cell_letters("BIC\nP S S T F R P P N T E"), None).as_deref(),
            Some("PSSTFRPPNTE")
        );
    }

    /// Avec l'IBAN, le registre dit quel code établissement attendre : un BIC d'une
    /// autre banque, ou lu à une lettre près sur ses quatre premières, est écarté.
    #[test]
    fn bic_must_match_the_bank_of_the_iban() {
        // 42529 = Edmond de Rothschild, BIC COFIFRCPXXX au registre
        let iban = "FR7642529000010000000000000";
        assert_eq!(
            extract_fr_bic("BIC COFIFRCPXXX", Some(iban)).as_deref(),
            Some("COFIFRCPXXX")
        );
        // un BIC d'une autre banque sur la page — ou un faux — est rejeté
        assert_eq!(extract_fr_bic("BIC SOGEFRPP", Some(iban)), None);
        // sans IBAN, pas de filtre
        assert_eq!(
            extract_fr_bic("BIC SOGEFRPP", None).as_deref(),
            Some("SOGEFRPP")
        );
    }

    #[test]
    fn bic_extraction_still_works_without_an_iban() {
        assert_eq!(
            extract_fr_bic("BIC AGRIFRPP847", None).as_deref(),
            Some("AGRIFRPP847")
        );
        assert_eq!(
            extract_fr_bic("BIC BOUS FRPP XXX", None).as_deref(),
            Some("BOUS FRPP XXX")
        );
    }

    #[test]
    fn test_normalize_iban() {
        assert_eq!(normalize_iban(IBAN), "FR7630001000644919009562088");
        assert_eq!(normalize_iban("fr76|3000 1000"), "FR7630001000");
    }

    fn to_rib(path: &str) -> Rib {
        let layout_text = std::fs::read_to_string(path).unwrap();
        Rib::parse(layout_text).unwrap_or_else(|| {
            panic!("Failed to parse RIB from file: {}", path);
        })
    }

    fn test_file(path: &str, account_holder: Option<Vec<&str>>, iban: &str, bic: &str) {
        let account_holder = account_holder.map(|v| v.join("\n"));
        assert_eq!(
            to_rib(path),
            Rib {
                account_holder,
                iban: iban.to_string(),
                bic: Some(bic.to_string()),
                bank_name: None
            }
        );
    }

    static IBAN: &str = "FR76 3000 1000 6449 1900 9562 088";

    #[test]
    fn rib_banque_populaire() {
        let path = "tests/fixtures/rib/banque_populaire.txt";
        let account_holder = Some(vec![
            "M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44100 NANTES",
        ]);
        let bic = "BDFEFRPPCCT";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_banque_populaire_2() {
        let path = "tests/fixtures/rib/banque_populaire_2.txt";
        let account_holder = Some(vec![
            "M HENRI MATISSE OU MLLE",
            "FRIDA KAHLO",
            "31 AVENUE JULES RENARD",
            "44800 ST HERBLAIN",
        ]);
        let bic = "BDFEFRPPCCT";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_banque_postale() {
        let path = "tests/fixtures/rib/banque_postale.txt";
        let account_holder = Some(vec![
            "MR MATISSE HENRI",
            "243 RUE DES GRIVES",
            "44240 LA CHAPELLE SUR ERDRE",
        ]);
        let bic = "PSSTFRPPNTE";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_banque_postale_2() {
        let path = "tests/fixtures/rib/banque_postale_2.txt";
        let account_holder = Some(vec!["MLE FRIDA KHALO", "OU MR MATISSE HENRI"]);
        let bic = "PSSTFRPPNTE";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_bourso() {
        let path = "tests/fixtures/rib/bourso.txt";
        let account_holder = Some(vec![
            "Mlle Kahlo Frida",
            "55 CHEMIN DU PETIT BOIS",
            "44400 REZE",
        ]);
        let bic = "BOUS FRPP XXX";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_caisse_epargne() {
        let path = "tests/fixtures/rib/caisse_epargne.txt";
        let account_holder = Some(vec![
            "MME KAHLO FRIDA OU M MATISSE",
            "143 ALLEE DES SALICAIRES",
            "44240 LA CHAPELLE SUR ERDRE",
        ]);
        let bic = "CEPAFRPP444";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_caisse_epargne_2() {
        let path = "tests/fixtures/rib/caisse_epargne_2.txt";
        let account_holder = Some(vec![
            "M MATISSE HENRI",
            "12 RUE VICTOR FORTUN",
            "44400 REZE",
        ]);
        let bic = "CEPAFRPP444";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_caisse_epargne_3() {
        let path = "tests/fixtures/rib/caisse_epargne_3.txt";
        let account_holder = Some(vec![
            "ML KHALO FRIDA OU M MATISSE H",
            "35 RUE DU CEDRE",
            "44240 LA CHAPELLE SUR ERDRE",
        ]);
        let bic = "CEPAFRPP444";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_credit_agricole() {
        let path = "tests/fixtures/rib/credit_agricole.txt";
        let account_holder = Some(vec![
            "MR OU MME MATISSE",
            "HENRI",
            "32 RUE EDOUARD TRAVIES",
            "44240 LA CHAPELLE SUR ERDRE",
        ]);
        let bic = "AGRIFRPP847";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_credit_agricole_2() {
        let path = "tests/fixtures/rib/credit_agricole_2.txt";
        let account_holder = Some(vec!["MME KAHLO FRIDA"]);
        let bic = "AGRIFRPP847";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_credit_agricole_3() {
        let path = "tests/fixtures/rib/credit_agricole_3.txt";
        let account_holder = Some(vec![
            "MLLE FRIDA KHALO",
            "15 RUE MARYSE BASTIE",
            "44230 ST SEBASTIEN SUR LOIRE",
        ]);
        let bic = "AGRIFRPP847";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_credit_mutuel() {
        let path = "tests/fixtures/rib/credit_mutuel.txt";
        let account_holder = Some(vec![
            "M HENRI MATISSE",
            "123 ALLEE DES ROSES",
            "44640 LE PELLERIN",
        ]);
        let bic = "CMCIFR2A";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_credit_mutuel_2() {
        let path = "tests/fixtures/rib/credit_mutuel_2.txt";
        let account_holder = Some(vec![
            "M OU MME MATISSE HENRI",
            "54 RUE DE L HERONNIERE",
            "44000 NANTES",
        ]);
        let bic = "CMBRFR2BXXX";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_fortuneo() {
        let path = "tests/fixtures/rib/fortuneo.txt";
        let account_holder = Some(vec!["Madame Khalo Frida ou Monsieur Matisse Henri"]);
        let bic = "FTNOFRP1XXX";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_lcl() {
        let path = "tests/fixtures/rib/lcl.txt";
        let account_holder = Some(vec!["M MATISSE HENRI"]);
        let bic = "CRLYFRPP";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_lcl_2() {
        let path = "tests/fixtures/rib/lcl_2.txt";
        let account_holder = Some(vec!["MLLE FRIDA KHALO"]);
        let bic = "CRLYFRPP";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_sg() {
        let path = "tests/fixtures/rib/sg.txt";
        let account_holder = Some(vec![
            "Mlle Frida Khalo",
            "117 rue des bourdonnieres 204 batiment c",
            "44200 Nantes",
        ]);
        let bic = "SOGEFRPP";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_sg_2() {
        let path = "tests/fixtures/rib/sg_2.txt";
        let account_holder = Some(vec![
            "SAS HENRI MATISSE",
            "18 RUE SADI CARNOT",
            "92120 MONTROUGE",
        ]);
        let bic = "SOGEFRPP";
        test_file(path, account_holder, IBAN, bic);
    }

    #[test]
    fn rib_orange() {
        let path = "tests/fixtures/rib/orange.txt";
        let account_holder = Some(vec!["M Matisse Henri"]);
        let bic = "GPBAFRPPXXX";
        test_file(path, account_holder, IBAN, bic);
    }
}
