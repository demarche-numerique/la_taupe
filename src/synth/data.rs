//! Données fictives d'un RIB de synthèse.
//!
//! Les IBAN produits sont structurellement valides (mod-97 et clé RIB) mais tirés au
//! hasard : ils ne désignent aucun compte réel. Les titulaires reprennent la convention
//! des fixtures existantes (peintres), pour qu'un RIB synthétique reste identifiable
//! comme tel au premier coup d'œil.

use crate::rib::{iban_from_bban, rib_key};

use super::rng::Rng;

/// Codes banque et raisons sociales réels, issus du fichier RIAD de la BCE : c'est la
/// même source que `IbanToBankName`, donc `bank_name` résout sur les IBAN générés.
const RIAD_CSV: &str = include_str!("../riad_bank_name.csv");

const BICS: [&str; 11] = [
    "BDFEFRPPCCT",
    "PSSTFRPPNTE",
    "BOUSFRPPXXX",
    "CEPAFRPP444",
    "AGRIFRPP847",
    "CMCIFR2A",
    "CMBRFR2BXXX",
    "FTNOFRP1XXX",
    "CRLYFRPP",
    "SOGEFRPP",
    "GPBAFRPPXXX",
];

const CIVILITIES: [&str; 8] = ["M", "MR", "MME", "MLLE", "MLE", "ML", "Monsieur", "Madame"];

const FIRST_NAMES: [&str; 10] = [
    "HENRI", "FRIDA", "CAMILLE", "PAUL", "BERTHE", "VINCENT", "SUZANNE", "GUSTAVE", "ROSA", "EDGAR",
];

const LAST_NAMES: [&str; 10] = [
    "MATISSE", "KAHLO", "CLAUDEL", "CEZANNE", "MORISOT", "VAN GOGH", "VALADON", "COURBET",
    "BONHEUR", "DEGAS",
];

const COMPANY_FORMS: [&str; 6] = ["SAS", "SARL", "SCI", "EURL", "ASSOCIATION", "SA"];

const STREETS: [&str; 10] = [
    "RUE BERNARD ROY",
    "AVENUE JULES RENARD",
    "RUE DES GRIVES",
    "CHEMIN DU PETIT BOIS",
    "ALLEE DES SALICAIRES",
    "RUE VICTOR FORTUN",
    "RUE EDOUARD TRAVIES",
    "RUE MARYSE BASTIE",
    "ALLEE DES ROSES",
    "RUE DE L HERONNIERE",
];

const CITIES: [(&str, &str); 8] = [
    ("44100", "NANTES"),
    ("44800", "ST HERBLAIN"),
    ("44240", "LA CHAPELLE SUR ERDRE"),
    ("44400", "REZE"),
    ("44640", "LE PELLERIN"),
    ("44230", "ST SEBASTIEN SUR LOIRE"),
    ("92120", "MONTROUGE"),
    ("44000", "NANTES"),
];

pub struct Bank {
    pub code: String,
    pub name: String,
    pub bic: String,
}

/// Un RIB de synthèse et sa vérité terrain.
pub struct RibData {
    pub bank: Bank,
    pub branch: String,
    pub account: String,
    pub rib_key: String,
    pub iban: String,
    /// Lignes du titulaire, dans l'ordre : c'est exactement ce que le pipeline doit
    /// retrouver, joint par des sauts de ligne.
    pub holder_lines: Vec<String>,
    /// Adresse de l'agence : leurre délibéré, le pipeline ne doit pas la confondre
    /// avec celle du titulaire.
    pub branch_lines: Vec<String>,
}

impl RibData {
    pub fn holder(&self) -> String {
        self.holder_lines.join("\n")
    }

    /// Le bloc « Code banque / Code guichet / N° compte / Clé RIB » du tableau, qui
    /// redit le BBAN de l'IBAN.
    pub fn bban(&self) -> String {
        format!(
            "{}{}{}{}",
            self.bank.code, self.branch, self.account, self.rib_key
        )
    }
}

fn banks() -> Vec<(String, String)> {
    RIAD_CSV
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let riad = fields.next()?;
            let name = fields.next()?;

            // le code RIAD est le code pays suivi des 5 chiffres du code banque
            riad.strip_prefix("FR")
                .filter(|code| code.len() == 5 && code.chars().all(|c| c.is_ascii_digit()))
                .map(|code| (code.to_string(), name.to_string()))
        })
        .collect()
}

fn address(rng: &mut Rng) -> Vec<String> {
    let (postal_code, city) = rng.pick(&CITIES);

    vec![
        format!("{} {}", rng.int(1, 250), rng.pick(&STREETS)),
        format!("{} {}", postal_code, city),
    ]
}

/// Titulaire : personne seule, couple joint par « OU », ou personne morale.
fn holder(rng: &mut Rng) -> Vec<String> {
    let mut lines = if rng.chance(0.15) {
        vec![format!(
            "{} {} {}",
            rng.pick(&COMPANY_FORMS),
            rng.pick(&FIRST_NAMES),
            rng.pick(&LAST_NAMES)
        )]
    } else if rng.chance(0.3) {
        // couple : parfois sur une ligne, parfois coupé au milieu comme sur les vrais RIB
        let first = format!(
            "{} {} {}",
            rng.pick(&CIVILITIES),
            rng.pick(&LAST_NAMES),
            rng.pick(&FIRST_NAMES)
        );
        let second = format!(
            "OU {} {} {}",
            rng.pick(&CIVILITIES),
            rng.pick(&LAST_NAMES),
            rng.pick(&FIRST_NAMES)
        );

        if rng.chance(0.5) {
            vec![format!("{} {}", first, second)]
        } else {
            vec![first, second]
        }
    } else {
        vec![format!(
            "{} {} {}",
            rng.pick(&CIVILITIES),
            rng.pick(&LAST_NAMES),
            rng.pick(&FIRST_NAMES)
        )]
    };

    // la plupart des RIB portent l'adresse du titulaire, mais pas tous
    if rng.chance(0.8) {
        lines.extend(address(rng));
    }

    lines
}

/// Numéro de compte : 11 caractères, parfois alphanumériques comme chez certaines
/// banques — ce qui exerce la table de conversion des lettres de la clé RIB.
fn account_number(rng: &mut Rng) -> String {
    if rng.chance(0.2) {
        let mut account = rng.digits(10);
        account.push(char::from(b'A' + rng.below(26) as u8));
        account
    } else {
        rng.digits(11)
    }
}

pub fn generate(rng: &mut Rng) -> RibData {
    let banks = banks();
    let (code, name) = rng.pick(&banks).clone();

    let branch = rng.digits(5);
    let account = account_number(rng);

    let key = rib_key(&code, &branch, &account).expect("composants de BBAN valides");
    let rib_key = format!("{:02}", key);

    let bban = format!("{}{}{}{}", code, branch, account, rib_key);
    let iban = iban_from_bban(&bban).expect("BBAN de 23 caractères");

    let mut branch_lines = vec![format!("AGENCE DE {}", rng.pick(&CITIES).1)];
    branch_lines.extend(address(rng));

    RibData {
        bank: Bank {
            code,
            name,
            bic: rng.pick(&BICS).to_string(),
        },
        branch,
        account,
        rib_key,
        iban,
        holder_lines: holder(rng),
        branch_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fi_extract::IbanToBankName;
    use crate::rib::check_rib_key;
    use iban::Iban;

    #[test]
    fn generated_ibans_are_structurally_valid() {
        let mut rng = Rng::new(1);

        for _ in 0..200 {
            let data = generate(&mut rng);

            assert!(
                data.iban.parse::<Iban>().is_ok(),
                "mod-97 invalide : {}",
                data.iban
            );
            assert!(
                check_rib_key(&data.iban),
                "clé RIB invalide : {}",
                data.iban
            );

            // le tableau RIB et l'IBAN portent bien la même information
            assert_eq!(data.iban[4..], data.bban());
        }
    }

    #[test]
    fn generated_bank_codes_resolve_to_a_name() {
        let mut rng = Rng::new(2);
        let names = IbanToBankName::new();

        for _ in 0..50 {
            let data = generate(&mut rng);
            assert!(names.bank_name(&data.iban).is_some());
        }
    }

    #[test]
    fn generation_is_reproducible() {
        let first = generate(&mut Rng::new(99)).iban;
        let second = generate(&mut Rng::new(99)).iban;

        assert_eq!(first, second);
    }
}
