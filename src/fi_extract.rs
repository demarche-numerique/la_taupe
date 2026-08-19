use std::collections::HashMap;

// fetch from https://www.ecb.europa.eu/stats/financial_corporations/list_of_financial_institutions/html/monthly_list-MID.en.html
// then iconv -f UTF-16 -t UTF-8 fi_mrr_csv_250630.csv | awk -F'\t' 'NR>1 && $1 ~ /^FR/ {print $1"\t"$2"\t"$4}' > src/riad_bank_name.csv
// columns: RIAD code (FR + 5-digit bank code), BIC (may be empty), name
const RIAD_CSV: &str = include_str!("./riad_bank_name.csv");

pub struct IbanToBankName {
    /// code banque → (BIC de l'établissement s'il est connu, nom)
    data: HashMap<String, (Option<String>, String)>,
}

impl IbanToBankName {
    pub fn new() -> Self {
        let mut data = HashMap::new();

        for line in RIAD_CSV.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() >= 3 {
                let riad_code = fields[0].to_string();
                let bic = Some(fields[1].trim().to_string()).filter(|b| !b.is_empty());
                let name = fields[2].to_string();
                data.insert(riad_code, (bic, name));
            }
        }

        Self { data }
    }

    fn riad_code(iban: &str) -> String {
        let iban_without_space = iban.replace(" ", "");
        let country_code = iban_without_space.chars().take(2).collect::<String>();
        let bank_code = iban_without_space
            .chars()
            .skip(4)
            .take(5)
            .collect::<String>();
        format!("{}{}", country_code, bank_code)
    }

    pub fn bank_name(&self, iban: &str) -> Option<String> {
        self.data
            .get(&Self::riad_code(iban))
            .map(|(_, name)| name.clone())
    }

    /// Code établissement attendu du BIC — ses quatre premières lettres — d'après le
    /// code banque de l'IBAN. Le BIC d'un RIB n'est pas toujours celui du siège
    /// (AGRIFRPP847 sur le RIB, AGRIFRCC847 au registre), mais les quatre premières
    /// lettres, elles, concordent. Inconnu pour les établissements sans BIC au registre
    /// — environ quatre sur dix.
    pub fn expected_bic_prefix(&self, iban: &str) -> Option<String> {
        self.data
            .get(&Self::riad_code(iban))
            .and_then(|(bic, _)| bic.as_ref())
            .map(|bic| bic.chars().take(4).collect())
    }
}

impl Default for IbanToBankName {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_bic_and_name() {
        let fi_extract = IbanToBankName::new();

        let result = fi_extract.bank_name("FR0042529ANDSTUFF");
        assert_eq!(result, Some("Edmond de Rothschild (France)".to_string()));

        // la colonne BIC du registre donne le code établissement attendu
        assert_eq!(
            fi_extract.expected_bic_prefix("FR0042529ANDSTUFF"),
            Some("COFI".to_string())
        );
        // établissement sans BIC au registre
        assert_eq!(fi_extract.expected_bic_prefix("FR0014228ANDSTUFF"), None);

        // Test avec un RIAD_CODE inexistant
        let result = fi_extract.bank_name("NONEXISTENT");
        assert_eq!(result, None);
    }
}
