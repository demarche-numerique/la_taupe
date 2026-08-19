use regex::Regex;

use super::patch::Patch;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum AddrType {
    Titulaire,
    Domiciliation,
    Unknown,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Addr {
    pub inner_lines: Vec<String>,
    pub addr_type: AddrType,
}

/// Une raison sociale peut précéder la civilité et fait partie du titulaire.
fn is_legal_form(line: &str) -> bool {
    Regex::new(r"(?i)^\s*(SCI|SAS|SASU|SARL|EURL|SA|SNC|SC|ASSOCIATION|ASS|GIE|SEL|SELARL|EARL|GAEC|SCP|SCM|SCEA)\b")
        .unwrap()
        .is_match(line)
}

fn civility_index(lines: &[String]) -> Option<usize> {
    let civility =
        Regex::new(r"(?i)(^|\s)(m|monsieur|mr|mademoiselle|ml|mle|mlle|melle|madame|mme)\.?\s")
            .unwrap();

    lines.iter().position(|line| civility.is_match(line))
}

impl Addr {
    pub fn lines(&self) -> Vec<String> {
        let header = Regex::new(r"(?i)(titulaire|intitulé|identit[e|é] bancaire)").unwrap();
        let intitule = Regex::new(r"(?i)(intitulé du compte)").unwrap();

        // Le bloc est remonté depuis le code postal jusqu'à un mot-clé ou six lignes.
        // Sans mot-clé, il avale ce qui précède dans la même colonne : référence
        // client, date d'édition, nom d'agence. La civilité marque le début réel du
        // titulaire ; seule une raison sociale placée juste au-dessus lui appartient.
        let start = civility_index(&self.inner_lines)
            .map(|i| {
                if i > 0 && is_legal_form(&self.inner_lines[i - 1]) {
                    i - 1
                } else {
                    i
                }
            })
            .unwrap_or(0);

        self.inner_lines[start..]
            .iter()
            .cloned()
            .map(|line| {
                if line.contains(':') {
                    line.split(':').nth(1).unwrap().to_string()
                } else {
                    line
                }
            })
            .map(|line| intitule.replace_all(&line, "").to_string())
            .filter(|line| !header.is_match(line))
            .map(|line| line.trim().to_string())
            .collect()
    }
}

pub fn find_account_holder_addr(text: &str) -> Option<Addr> {
    let lines: Vec<&str> = text.split("\n").collect();
    // Le code postal peut être collé à la ville ou séparé d'elle par un tiret : les
    // chaînes éditiques bancaires produisent l'un comme l'autre. Sans cette tolérance,
    // le bloc n'est pas ancré et le repli sur le libellé le tronque.
    let code_postal = Regex::new(r"(^| )\d{5}( ?- ?| ?)([[:alpha:]]+ ?)+").unwrap();
    let patch_upper_limit =
        Regex::new(r"(?i)(titulaire|intitulé|domiciliation|cadre réservé)").unwrap();

    // Deux blocs adressés côte à côte — titulaire d'un côté, agence de l'autre — mettent
    // deux codes postaux sur la même ligne. N'examiner que le premier, c'est ne voir que
    // le bloc de gauche, et donc rater le titulaire dès qu'il est à droite.
    let patches = lines
        .clone()
        .into_iter()
        .enumerate()
        .flat_map(|(index, line)| {
            code_postal
                .find_iter(line)
                .map(|m| {
                    Patch::extract(
                        &lines,
                        index,
                        &patch_upper_limit,
                        m.start(),
                        m.end() - 1,
                        true,
                        3,
                    )
                })
                .collect::<Vec<Patch>>()
        });

    let addresses = patches.map(|p| {
        let addr_type = addr_type_from_text(&p);
        Addr {
            inner_lines: p.lines(),
            addr_type,
        }
    });

    let addr = addresses
        .clone()
        .filter(|addr| addr.addr_type == AddrType::Titulaire)
        .collect::<Vec<Addr>>()
        .first()
        .cloned();

    if let Some(addr) = addr {
        return Some(addr);
    }
    let addr = addresses
        .filter(|addr| addr.addr_type == AddrType::Unknown)
        .collect::<Vec<Addr>>()
        .first()
        .cloned();

    addr
}

fn addr_type_from_text(patch: &Patch) -> AddrType {
    let account_holder = Regex::new(r"(?i)(titulaire|intitulé)").unwrap();
    let domiciliation = Regex::new(r"(?i)(domiciliation|cadre réservé)").unwrap();

    let text = patch.lines().join(" ");

    if account_holder.is_match(&text) {
        return AddrType::Titulaire;
    } else if domiciliation.is_match(&text) {
        return AddrType::Domiciliation;
    }

    let context = patch.context_lines.join(" ");

    if account_holder.is_match(&context) {
        return AddrType::Titulaire;
    } else if domiciliation.is_match(&context) {
        return AddrType::Domiciliation;
    }
    AddrType::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vec_to_string(v: Vec<&str>) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn test_file(path: &str, account_holder: Vec<&str>) {
        let layout_text = std::fs::read_to_string(path).unwrap();
        let account_holder = Some(vec_to_string(account_holder));
        let addrs = find_account_holder_addr(&layout_text);
        assert_eq!(addrs.map(|a| a.lines()), account_holder);
    }

    #[test]
    fn addr_banque_populaire() {
        let path = "tests/fixtures/rib/banque_populaire.txt";
        let account_holder = vec![
            "M OU MME MATISSE HENRI",
            "51 RUE BERNARD ROY",
            "44100 NANTES",
        ];

        test_file(path, account_holder);
    }

    #[test]
    fn addr_banque_postale() {
        let path = "tests/fixtures/rib/banque_postale.txt";
        let account_holder = vec![
            "MR MATISSE HENRI",
            "243 RUE DES GRIVES",
            "44240 LA CHAPELLE SUR ERDRE",
        ];

        test_file(path, account_holder);
    }

    /// Les chaînes éditiques collent parfois le code postal à la ville, ou les séparent
    /// d'un tiret. Le bloc doit rester ancré.
    #[test]
    fn postal_code_glued_or_dashed_still_anchors_the_block() {
        let glued = "Titulaire du compte\nM MATISSE HENRI\n51 RUE BERNARD ROY\n44100NANTES";
        let dashed = "Titulaire du compte\nM MATISSE HENRI\n51 RUE BERNARD ROY\n44100 - NANTES";

        assert_eq!(
            find_account_holder_addr(glued).map(|a| a.lines()),
            Some(vec_to_string(vec![
                "M MATISSE HENRI",
                "51 RUE BERNARD ROY",
                "44100NANTES"
            ]))
        );
        assert_eq!(
            find_account_holder_addr(dashed).map(|a| a.lines()),
            Some(vec_to_string(vec![
                "M MATISSE HENRI",
                "51 RUE BERNARD ROY",
                "44100 - NANTES"
            ]))
        );
    }

    /// Titulaire à droite d'une domiciliation qui porte elle aussi un code postal : les
    /// deux blocs mettent deux codes postaux sur la même ligne, et seul le premier était
    /// examiné. Le titulaire était systématiquement raté dès qu'il était à droite.
    #[test]
    fn a_holder_to_the_right_of_a_branch_address_is_found() {
        let text = "\
  Domiciliation                             Titulaire du compte (Account Owner)
  CIC NANTES VOLTAIRE                       MLE PRENOM NOM
  4 RUE VOLTAIRE                            9 RUE UN NOM EN PLUSIEURS MOT
  44000 NANTES                              44100 NANTES
  tel";

        assert_eq!(
            find_account_holder_addr(text).map(|a| a.lines()),
            Some(vec_to_string(vec![
                "MLE PRENOM NOM",
                "9 RUE UN NOM EN PLUSIEURS MOT",
                "44100 NANTES"
            ]))
        );
    }

    /// Sans mot-clé au-dessus du bloc, la remontée avalait la référence client et la
    /// date d'édition posées dans la même colonne. La civilité borne le titulaire.
    #[test]
    fn lines_above_the_civility_are_dropped() {
        let text = "Ref client 0123456789\nEdite le 12/03/2025\n\nM MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES";

        assert_eq!(
            find_account_holder_addr(text).map(|a| a.lines()),
            Some(vec_to_string(vec![
                "M MATISSE HENRI",
                "51 RUE BERNARD ROY",
                "44100 NANTES"
            ]))
        );
    }

    /// Une raison sociale juste au-dessus de la civilité fait partie du titulaire.
    #[test]
    fn a_legal_form_above_the_civility_is_kept() {
        let text = "Titulaire\nSCI LES TILLEULS\nM MATISSE HENRI\n51 RUE BERNARD ROY\n44100 NANTES";

        assert_eq!(
            find_account_holder_addr(text).map(|a| a.lines()),
            Some(vec_to_string(vec![
                "SCI LES TILLEULS",
                "M MATISSE HENRI",
                "51 RUE BERNARD ROY",
                "44100 NANTES"
            ]))
        );
    }

    /// Sans civilité — personne morale seule — rien n'est retiré.
    #[test]
    fn without_civility_the_block_is_untouched() {
        let text = "Titulaire\nSAS HENRI MATISSE\n18 RUE SADI CARNOT\n92120 MONTROUGE";

        assert_eq!(
            find_account_holder_addr(text).map(|a| a.lines()),
            Some(vec_to_string(vec![
                "SAS HENRI MATISSE",
                "18 RUE SADI CARNOT",
                "92120 MONTROUGE"
            ]))
        );
    }

    #[test]
    fn addr_credit_mutuel_2() {
        let path = "tests/fixtures/rib/credit_mutuel_2.txt";
        let account_holder = vec![
            "M OU MME MATISSE HENRI",
            "54 RUE DE L HERONNIERE",
            "44000 NANTES",
        ];

        test_file(path, account_holder);
    }
}
