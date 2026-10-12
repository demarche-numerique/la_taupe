//! Le bloc titulaire en deux parties : le nom, qu'un appelant rapproche de celui d'un
//! dossier, et l'adresse.
//!
//! L'adresse commence à la première ligne, sous le nom, qui en porte une marque : un
//! numéro, un type de voie, un complément, une boîte postale, un code postal. Tout ce
//! qui suit lui appartient, même sans marque — un nom de bâtiment entre la rue et la
//! ville, une ville seule sous son code postal. Une ligne sans marque au-dessus — un
//! lieu-dit, un service, une résidence sans mot-clé — reste au nom : un mot d'adresse
//! en trop gêne peu un rapprochement, un nom passé dans l'adresse le fait échouer.

use regex::Regex;

/// Un code postal suivi d'une ville — pas une rangée de chiffres du RIB.
fn postal() -> Regex {
    Regex::new(r"(^|\s)(F-)?\d{5}\s*[[:alpha:]]{2,}").unwrap()
}

/// Une ligne d'adresse commence par un numéro, ou par un mot de voie, de complément ou
/// de distribution, éventuellement précédé du numéro. Les abréviations qui sont aussi
/// des initiales (« R », « CH », « PL ») sont écartées.
fn address_line() -> Regex {
    Regex::new(
        r"(?i)^\s*(\d{1,5}\b|b\.\s?p\.|(rue|avenue|av|ave|bd|boulevard|all[ée]es?|chemin|place|impasse|imp|route|rte|quai|cours|square|villa|lieu[- ]?dit|quartier|r[ée]sidence|r[ée]s|lotissement|chauss[ée]e|faubourg|fbg|passage|parvis|esplanade|sentier|hameau|cit[ée]|domaine|mont[ée]e|voie|promenade|clos|traverse|zone|zi|za|zac|parc|b[aâ]t|b[aâ]timent|appartement|appt|apt|[ée]tage|escalier|esc|immeuble|bp|cs|tsa|bo[iî]te\s+postale|chez|c/o|cedex)\b|france\s*$)",
    )
    .unwrap()
}

/// Le début d'un nom : une civilité, une forme juridique.
fn name_start() -> Regex {
    Regex::new(
        r"(?i)^\s*((m|mr|monsieur|mme|madame|mlle|mle|ml|melle|mademoiselle)\.?\s|(sci|sas|sasu|sarl|eurl|sa|snc|sc|scp|scm|scea|selarl|earl|gaec|gie|association|assoc|ass)\b)",
    )
    .unwrap()
}

/// La suite d'un nom sur la ligne d'après : « OU MME … », « ET … », « NEE … ».
fn name_continuation() -> Regex {
    Regex::new(r"(?i)^\s*(ou|et|&|n[ée]e|ep|[ée]pouse|veuve|vve)\b").unwrap()
}

/// L'adresse collée au nom sur la même ligne, comme le rend parfois la couche texte
/// d'un PDF : un numéro suivi d'un type de voie, ou un code postal.
fn address_inside() -> Regex {
    Regex::new(
        r"(?i)\s(\d{1,4}\s*(bis|ter)?\s+(rue|avenue|av|bd|boulevard|all[ée]es?|chemin|place|impasse|route|quai|cours|square|r[ée]sidence|lotissement|faubourg|passage|hameau|cit[ée]|domaine)\b|\d{5}\s+[[:alpha:]]{2,})",
    )
    .unwrap()
}

fn is_name_line(line: &str) -> bool {
    name_start().is_match(line) || name_continuation().is_match(line)
}

fn is_address_line(line: &str) -> bool {
    !is_name_line(line) && (postal().is_match(line) || address_line().is_match(line))
}

/// Coupe une ligne de nom où l'adresse commence, si elle y est collée. Une raison
/// sociale qui porte une adresse (« SCI 2 RUE DES LILAS ») reste entière : il ne
/// resterait du nom que sa forme juridique.
fn split_inside(line: &str) -> (String, Option<String>) {
    if let Some(m) = address_inside().find(line) {
        let name = line[..m.start()].trim();
        let bare = name_start()
            .find(name)
            .is_some_and(|s| s.end() >= name.len());
        if !name.is_empty() && !bare {
            return (name.to_string(), Some(line[m.start()..].trim().to_string()));
        }
    }
    (line.to_string(), None)
}

fn joined(lines: Vec<String>) -> Option<String> {
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Sépare un bloc titulaire (lignes séparées par `\n`) en `(nom, adresse)`.
pub fn split_holder(holder: &str) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = holder
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return (None, None);
    }

    // la première ligne est le nom : un bloc qui n'en a pas est déjà un échec, et la
    // lire comme une adresse ne rend rien de meilleur — sauf seule, et adresse
    let first = if lines.len() > 1 { 1 } else { 0 };
    let start = (first..lines.len())
        .find(|&i| is_address_line(lines[i]))
        .unwrap_or(lines.len());

    let mut name: Vec<String> = Vec::new();
    let mut address: Vec<String> = Vec::new();
    for line in &lines[..start] {
        let (kept, rest) = split_inside(line);
        match rest {
            // ce qui suit une adresse collée appartient à l'adresse
            Some(rest) => {
                name.push(kept);
                address.push(rest);
            }
            None if address.is_empty() => name.push(kept),
            None => address.push(kept),
        }
    }
    address.extend(lines[start..].iter().map(|l| l.to_string()));

    (joined(name), joined(address))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(holder: &[&str]) -> (Option<String>, Option<String>) {
        split_holder(&holder.join("\n"))
    }

    fn expect(name: &[&str], address: &[&str]) -> (Option<String>, Option<String>) {
        let join = |v: &[&str]| (!v.is_empty()).then(|| v.join("\n"));
        (join(name), join(address))
    }

    #[test]
    fn name_above_street_and_city() {
        assert_eq!(
            parts(&["M MATISSE HENRI", "51 RUE DES GRIVES", "44100 NANTES"]),
            expect(&["M MATISSE HENRI"], &["51 RUE DES GRIVES", "44100 NANTES"])
        );
    }

    /// Un couple sur deux lignes : la seconde reste au nom, qu'elle commence par « OU »
    /// ou non.
    #[test]
    fn a_couple_on_two_lines_stays_in_the_name() {
        assert_eq!(
            parts(&[
                "MLE FRIDA KAHLO",
                "OU MR MATISSE HENRI",
                "3 RUE DES LILAS",
                "44000 NANTES"
            ]),
            expect(
                &["MLE FRIDA KAHLO", "OU MR MATISSE HENRI"],
                &["3 RUE DES LILAS", "44000 NANTES"]
            )
        );
        assert_eq!(
            parts(&[
                "MR OU MME MATISSE",
                "HENRI",
                "32 RUE DES GRIVES",
                "44240 LA CHAPELLE SUR ERDRE"
            ]),
            expect(
                &["MR OU MME MATISSE", "HENRI"],
                &["32 RUE DES GRIVES", "44240 LA CHAPELLE SUR ERDRE"]
            )
        );
        assert_eq!(
            parts(&[
                "M HENRI MATISSE OU MLLE",
                "FRIDA KAHLO",
                "31 AVENUE DES LILAS",
                "44800 ST HERBLAIN"
            ]),
            expect(
                &["M HENRI MATISSE OU MLLE", "FRIDA KAHLO"],
                &["31 AVENUE DES LILAS", "44800 ST HERBLAIN"]
            )
        );
    }

    #[test]
    fn complements_and_post_boxes_are_address() {
        assert_eq!(
            parts(&[
                "MME KAHLO FRIDA",
                "CHEZ M MATISSE",
                "RESIDENCE LES PINS",
                "BAT C APPT 12",
                "8 ALLEE DES CEDRES",
                "BP 42",
                "44000 NANTES CEDEX 1",
            ]),
            expect(
                &["MME KAHLO FRIDA"],
                &[
                    "CHEZ M MATISSE",
                    "RESIDENCE LES PINS",
                    "BAT C APPT 12",
                    "8 ALLEE DES CEDRES",
                    "BP 42",
                    "44000 NANTES CEDEX 1",
                ]
            )
        );
    }

    /// Une fois l'adresse commencée, une ligne sans marque lui appartient : un nom de
    /// bâtiment entre la rue et la ville, une ville seule sous son code postal.
    #[test]
    fn an_unmarked_line_inside_the_address_stays_there() {
        assert_eq!(
            parts(&[
                "ASSOCIATION DES PEINTRES",
                "15 RUE DES LILAS",
                "HOTEL DES ARTS",
                "44000 NANTES"
            ]),
            expect(
                &["ASSOCIATION DES PEINTRES"],
                &["15 RUE DES LILAS", "HOTEL DES ARTS", "44000 NANTES"]
            )
        );
        assert_eq!(
            parts(&["M MATISSE HENRI", "42 AVENUE DES LILAS", "44000", "NANTES"]),
            expect(
                &["M MATISSE HENRI"],
                &["42 AVENUE DES LILAS", "44000", "NANTES"]
            )
        );
    }

    #[test]
    fn dotted_post_boxes_and_quarters_are_address() {
        assert_eq!(
            parts(&["LYCEE DES ARTS", "B.P. 21 - RUE DES LILAS", "44000 NANTES"]),
            expect(
                &["LYCEE DES ARTS"],
                &["B.P. 21 - RUE DES LILAS", "44000 NANTES"]
            )
        );
        assert_eq!(
            parts(&[
                "ASSOCIATION DES PEINTRES",
                "QUARTIER DES LILAS",
                "44000 NANTES"
            ]),
            expect(
                &["ASSOCIATION DES PEINTRES"],
                &["QUARTIER DES LILAS", "44000 NANTES"]
            )
        );
    }

    /// Un sigle d'un seul mot devant la rue reste au nom.
    #[test]
    fn an_acronym_glued_to_the_street_is_cut_off() {
        assert_eq!(
            parts(&[
                "SOCIETE DES PEINTRES",
                "S.D.P 6 RUE DES LILAS",
                "44000 NANTES"
            ]),
            expect(
                &["SOCIETE DES PEINTRES", "S.D.P"],
                &["6 RUE DES LILAS", "44000 NANTES"]
            )
        );
    }

    /// Un lieu-dit sans mot-clé reste au nom : en cas de doute, le nom garde la ligne.
    #[test]
    fn an_unmarked_line_stays_in_the_name() {
        assert_eq!(
            parts(&[
                "M MATISSE HENRI",
                "LES HAUTS DE LA COLLINE",
                "44110 CHATEAUBRIANT"
            ]),
            expect(
                &["M MATISSE HENRI", "LES HAUTS DE LA COLLINE"],
                &["44110 CHATEAUBRIANT"]
            )
        );
    }

    #[test]
    fn a_legal_entity_keeps_its_lines() {
        assert_eq!(
            parts(&[
                "ASSOCIATION DES AMIS DE LA PEINTURE",
                "SECTION DE NANTES",
                "2 PLACE DU MARCHE",
                "44000 NANTES"
            ]),
            expect(
                &["ASSOCIATION DES AMIS DE LA PEINTURE", "SECTION DE NANTES"],
                &["2 PLACE DU MARCHE", "44000 NANTES"]
            )
        );
    }

    #[test]
    fn an_address_glued_to_the_name_is_cut_off() {
        assert_eq!(
            parts(&["MME KAHLO FRIDA 12 RUE DES LILAS 44000 NANTES"]),
            expect(&["MME KAHLO FRIDA"], &["12 RUE DES LILAS 44000 NANTES"])
        );
        assert_eq!(
            parts(&["M MATISSE HENRI 44000 NANTES"]),
            expect(&["M MATISSE HENRI"], &["44000 NANTES"])
        );
        // une raison sociale qui porte une adresse reste entière
        assert_eq!(
            parts(&["SCI 2 RUE DES LILAS", "2 RUE DES LILAS", "44000 NANTES"]),
            expect(
                &["SCI 2 RUE DES LILAS"],
                &["2 RUE DES LILAS", "44000 NANTES"]
            )
        );
    }

    #[test]
    fn without_address_everything_is_the_name() {
        assert_eq!(
            parts(&["MME KAHLO FRIDA"]),
            expect(&["MME KAHLO FRIDA"], &[])
        );
        assert_eq!(
            parts(&["MLE FRIDA KAHLO", "OU MR MATISSE HENRI"]),
            expect(&["MLE FRIDA KAHLO", "OU MR MATISSE HENRI"], &[])
        );
        assert_eq!(
            parts(&["Madame Kahlo Frida ou Monsieur Matisse Henri"]),
            expect(&["Madame Kahlo Frida ou Monsieur Matisse Henri"], &[])
        );
    }

    /// Une adresse sans code postal lu se reconnaît encore à sa voie.
    #[test]
    fn a_street_without_postal_code_is_address() {
        assert_eq!(
            parts(&["M MATISSE HENRI", "12 RUE DES LILAS"]),
            expect(&["M MATISSE HENRI"], &["12 RUE DES LILAS"])
        );
    }

    /// Ce qui suit le code postal (le pays) reste à l'adresse.
    #[test]
    fn the_country_after_the_city_is_address() {
        assert_eq!(
            parts(&[
                "M MATISSE HENRI",
                "12 RUE DES LILAS",
                "44000 NANTES",
                "FRANCE"
            ]),
            expect(
                &["M MATISSE HENRI"],
                &["12 RUE DES LILAS", "44000 NANTES", "FRANCE"]
            )
        );
    }

    /// Un bloc sans nom lisible garde sa première ligne au nom.
    #[test]
    fn the_first_line_is_never_left_without_name() {
        assert_eq!(
            parts(&["12 RUE DES LILAS", "44000 NANTES"]),
            expect(&["12 RUE DES LILAS"], &["44000 NANTES"])
        );
    }

    #[test]
    fn lower_case_lines_are_split_too() {
        assert_eq!(
            parts(&[
                "Mlle Frida Kahlo",
                "117 rue des lilas 204 batiment c",
                "44200 Nantes"
            ]),
            expect(
                &["Mlle Frida Kahlo"],
                &["117 rue des lilas 204 batiment c", "44200 Nantes"]
            )
        );
    }

    #[test]
    fn nothing_gives_nothing() {
        assert_eq!(split_holder(""), (None, None));
    }
}
