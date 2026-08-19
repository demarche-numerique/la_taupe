//! Référentiel code postal → commune, pour corriger la ligne de ville d'un titulaire.
//!
//! Sur les photos, la ville est la ligne la plus souvent hachée par l'OCR — « 44800STHER »,
//! « 44640LE PELLERIN » — alors que le code postal, cinq chiffres serrés, survit bien
//! mieux. Or un code postal ne correspond qu'à une poignée de communes : quand l'une
//! d'elles ressemble à ce qui a été lu, c'est elle. La correction ne touche que la ligne
//! ville, et seulement si le code postal est présent et la commune proche : on ne devine
//! pas, on recoupe.
//!
//! Source : Base officielle des codes postaux, La Poste, Licence Ouverte 2.0,
//! <https://www.data.gouv.fr/datasets/base-officielle-des-codes-postaux>. Seules les
//! colonnes code postal et libellé d'acheminement sont conservées — la forme postale en
//! capitales sans accent, celle qu'un RIB imprime. Regénérer :
//!
//!     curl -fsSL https://data.laposte.fr/data-fair/api/v1/datasets/laposte-hexasmal/raw \
//!       | iconv -f LATIN1 -t UTF-8 | awk -F';' 'NR>1 {print $3";"$4}' | sort -u \
//!       > src/code_postal_commune.csv

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

const CSV: &str = include_str!("../code_postal_commune.csv");

fn index() -> &'static HashMap<&'static str, Vec<&'static str>> {
    static INDEX: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();

    INDEX.get_or_init(|| {
        let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
        for line in CSV.lines() {
            if let Some((cp, commune)) = line.split_once(';') {
                map.entry(cp).or_default().push(commune);
            }
        }
        map
    })
}

/// Communes desservies par un code postal, dans leur libellé d'acheminement.
pub fn communes(code_postal: &str) -> &'static [&'static str] {
    index().get(code_postal).map(Vec::as_slice).unwrap_or(&[])
}

/// Distance de Levenshtein.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Ne garde que lettres et chiffres, en capitales : la forme sous laquelle on compare.
fn squash(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Corrige la ville d'une ligne « code postal ville » d'après le référentiel.
///
/// Rend la ligne réécrite quand le code postal est connu et qu'une de ses communes
/// ressemble à ce qui a été lu — même chaîne une fois les espaces retirés, ou distance
/// d'édition d'au plus un tiers de la longueur (deux caractères minimum). Sinon, ou si
/// le code postal est inconnu, rend `None` et la ligne reste telle quelle. Quand la
/// ville lue est vide mais que le code postal ne dessert qu'une commune, on la complète.
pub fn fix_city_line(line: &str) -> Option<String> {
    // la ville peut contenir un chiffre lu pour une lettre (« NANTE5 ») : on l'accepte,
    // la comparaison se fait sur la forme et le référentiel tranche
    let re = Regex::new(r"^(?P<before>.*?)(?P<cp>\d{5})\s*(?P<city>[A-Za-z0-9][^\n]*)?$").ok()?;
    let caps = re.captures(line.trim())?;
    let cp = caps.name("cp")?.as_str();
    let city_read = caps.name("city").map(|m| m.as_str().trim()).unwrap_or("");
    let before = caps.name("before").map(|m| m.as_str()).unwrap_or("");

    let candidates = communes(cp);
    if candidates.is_empty() {
        return None;
    }

    let read = squash(city_read);
    let best = if read.is_empty() {
        // pas de ville lue : on ne complète que si elle est sans ambiguïté
        (candidates.len() == 1).then(|| candidates[0])
    } else {
        // une ville tronquée est un préfixe du vrai nom : « STHER » pour ST HERBLAIN.
        // On l'accepte si elle ne désigne qu'une commune du code postal.
        let by_prefix: Vec<&str> = if read.chars().count() >= 4 {
            candidates
                .iter()
                .copied()
                .filter(|c| squash(c).starts_with(&read))
                .collect()
        } else {
            Vec::new()
        };
        if by_prefix.len() == 1 {
            Some(by_prefix[0])
        } else {
            candidates
                .iter()
                .map(|c| (edit_distance(&squash(c), &read), *c))
                .filter(|(d, c)| {
                    let len = squash(c).chars().count().max(read.chars().count());
                    // un code postal à commune unique lève l'ambiguïté : on tolère une
                    // lecture plus abîmée, jusqu'à la moitié des caractères, mais pas
                    // n'importe quoi — une ville sans rapport reste telle quelle
                    let tolerance = if candidates.len() == 1 {
                        len / 2
                    } else {
                        len / 3
                    };
                    *d == 0 || *d <= tolerance.max(2)
                })
                .min_by_key(|(d, _)| *d)
                .map(|(_, c)| c)
        }
    }?;

    let fixed = format!("{}{} {}", before, cp, best).trim().to_string();

    // déjà juste, à la casse près : la typographie du document prime, on ne réécrit que
    // ce qui est faux — un espace manquant l'est, une minuscule ne l'est pas
    if fixed.eq_ignore_ascii_case(line.trim()) {
        return None;
    }

    Some(fixed)
}

/// Applique `fix_city_line` à la dernière ligne portant un code postal d'un bloc
/// titulaire. Les autres lignes ne sont pas touchées.
pub fn fix_holder_city(holder: &str) -> String {
    let mut lines: Vec<String> = holder.lines().map(str::to_string).collect();
    let has_cp = Regex::new(r"(^|\D)\d{5}").unwrap();
    if let Some(i) = lines.iter().rposition(|l| has_cp.is_match(l)) {
        if let Some(fixed) = fix_city_line(&lines[i]) {
            lines[i] = fixed;
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_resolves_known_codes() {
        assert!(communes("44800").contains(&"ST HERBLAIN"));
        assert!(communes("44700").contains(&"ORVAULT"));
        assert!(communes("00000").is_empty());
    }

    /// Les trois formes observées sur photo : ville hachée, ville collée, ville lue à
    /// une lettre près.
    #[test]
    fn a_hashed_city_is_restored_from_the_postal_code() {
        assert_eq!(
            fix_city_line("44800STHER").as_deref(),
            Some("44800 ST HERBLAIN")
        );
        assert_eq!(
            fix_city_line("44640LE PELLERIN").as_deref(),
            Some("44640 LE PELLERIN")
        );
        assert_eq!(
            fix_city_line("44700 ORVAUL").as_deref(),
            Some("44700 ORVAULT")
        );
        assert_eq!(
            fix_city_line("44100 NANTE5").as_deref(),
            Some("44100 NANTES")
        );
    }

    /// On ne devine pas : une ville sans rapport avec le code postal reste telle quelle,
    /// un code postal inconnu aussi.
    /// Une ville juste en casse mixte n'est pas réécrite : la typographie du document
    /// prime, on ne corrige que ce qui est faux.
    #[test]
    fn a_correct_city_keeps_its_case() {
        assert_eq!(fix_city_line("44200 Nantes"), None);
        assert_eq!(fix_city_line("44800 St Herblain"), None);
        // mais collée, elle est réespacée
        assert_eq!(
            fix_city_line("44800STHERBLAIN").as_deref(),
            Some("44800 ST HERBLAIN")
        );
    }

    #[test]
    fn an_unrelated_city_is_left_alone() {
        assert_eq!(fix_city_line("44800 PARIS"), None);
        assert_eq!(fix_city_line("00000 NULLEPART"), None);
    }

    #[test]
    fn only_the_city_line_of_the_holder_is_touched() {
        let holder = "M MATISSE HENRI\n51 RUE BERNARD ROY\n44100NANTE5";
        assert_eq!(
            fix_holder_city(holder),
            "M MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES"
        );
        // sans code postal, rien ne bouge
        assert_eq!(fix_holder_city("M MATISSE HENRI"), "M MATISSE HENRI");
    }
}
