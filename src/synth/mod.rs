//! Génération d'un corpus de RIB synthétiques.
//!
//! Le corpus réel étant constitué de documents personnels, le développement des
//! algorithmes se fait sur des RIB fictifs dégradés de façon contrôlée. Chaque
//! dégradation étant paramétrée et nommée, le banc ne rend pas un taux global mais des
//! courbes : à quelle hauteur de capitale, à quel angle, à quel niveau de bruit la
//! reconnaissance cède.

pub mod data;
pub mod degrade;
pub mod layout;
pub mod package;
pub mod pdf;
pub mod rng;

use std::fs;
use std::path::Path;

use degrade::Degradation;
use package::{Form, Sample};
use rng::Rng;

#[derive(Clone)]
pub struct Scenario {
    pub form: Form,
    pub degradation: Degradation,
}

fn scan(cap_height: u32) -> Degradation {
    Degradation {
        cap_height,
        ..Default::default()
    }
}

/// Une photo prise au téléphone porte toujours un support visible et un éclairage
/// inégal — le document n'y est ni détouré, ni uniformément exposé. Il n'existe pas de
/// « photo propre » ; en simuler serait se donner des cas qui n'arrivent pas.
fn photo(cap_height: u32) -> Degradation {
    Degradation {
        cap_height,
        background: true,
        illumination: 0.4,
        blur_sigma: 0.5,
        jpeg_quality: Some(70),
        ..Default::default()
    }
}

/// La grille de conditions à couvrir.
///
/// Sa composition suit ce que le service reçoit réellement, établi sur deux corpus :
/// des photos d'un côté, des PDF produits par les chaînes éditiques bancaires de
/// l'autre. Les scans y sont marginaux, et ce sont les photos — non les scans — qui
/// arrivent pivotées.
///
/// Les PDF natifs restent peu nombreux malgré leur poids réel : ils sont reconnus sans
/// OCR, donc toujours à 100 %, et les sur-représenter diluerait les écarts que le banc
/// doit rendre visibles. Le taux global n'est pas une estimation du taux de production,
/// c'est un indicateur de régression.
///
/// Chaque paramètre est éprouvé séparément, pour qu'un échec s'impute à une cause et
/// non à un cumul.
pub fn scenarios() -> Vec<Scenario> {
    let jpeg = |degradation: Degradation| Scenario {
        form: Form::Jpeg,
        degradation,
    };

    let scanned = |degradation: Degradation| Scenario {
        form: Form::PdfImg,
        degradation,
    };

    vec![
        // référence : reconnu sans OCR
        Scenario {
            form: Form::PdfText,
            degradation: scan(30),
        },
        // photos : résolution, du plus lisible au plus petit texte observé
        jpeg(photo(45)),
        jpeg(photo(30)),
        jpeg(photo(20)),
        jpeg(photo(14)),
        // photos pivotées : un quart des photos réelles le sont
        jpeg(Degradation {
            quarter_turns: 1,
            ..photo(20)
        }),
        jpeg(Degradation {
            quarter_turns: 2,
            ..photo(20)
        }),
        jpeg(Degradation {
            quarter_turns: 3,
            ..photo(20)
        }),
        // photos : chaque défaut de prise de vue isolé
        jpeg(Degradation {
            rotation_deg: 3.0,
            ..photo(20)
        }),
        jpeg(Degradation {
            rotation_deg: 15.0,
            ..photo(20)
        }),
        jpeg(Degradation {
            perspective: 0.05,
            ..photo(20)
        }),
        jpeg(Degradation {
            blur_sigma: 1.5,
            ..photo(20)
        }),
        jpeg(Degradation {
            noise: 12.0,
            ..photo(20)
        }),
        jpeg(Degradation {
            illumination: 0.85,
            ..photo(20)
        }),
        jpeg(Degradation {
            jpeg_quality: Some(40),
            ..photo(20)
        }),
        // photo cumulant les défauts, prise à la va-vite
        jpeg(Degradation {
            rotation_deg: 5.0,
            perspective: 0.06,
            blur_sigma: 1.2,
            noise: 10.0,
            illumination: 0.8,
            jpeg_quality: Some(45),
            ..photo(14)
        }),
        // Photos sur lesquelles l'OCR ne détecte presque rien : c'est à cela que
        // ressemblent les échecs réels — une dizaine de caractères lus, aucun libellé,
        // pas de préfixe d'IBAN — et aucune recette ci-dessus ne le produisait. Le corpus
        // sous-estimait la part d'images irrécupérables et surestimait celle des
        // documents lisibles où seul l'IBAN manque.
        //
        // Deux recettes seulement : les images irrécupérables sont environ six pour cent
        // du corpus réel, et en mettre davantage écraserait le taux global sans rien
        // apprendre de plus.
        jpeg(Degradation {
            blur_sigma: 3.5,
            ..photo(10)
        }),
        jpeg(Degradation {
            perspective: 0.12,
            rotation_deg: 25.0,
            blur_sigma: 1.5,
            ..photo(12)
        }),
        // orientation portée par le tag EXIF plutôt que par les pixels
        Scenario {
            form: Form::JpegExif,
            degradation: photo(20),
        },
        Scenario {
            form: Form::Png,
            degradation: photo(30),
        },
        // scans : minoritaires, mais présents
        scanned(scan(20)),
        scanned(scan(14)),
        // conditionnements qui déroutent l'aiguillage de `vec_to_rib`
        Scenario {
            form: Form::PdfImgText,
            degradation: scan(20),
        },
        Scenario {
            form: Form::PdfImgMulti,
            degradation: scan(20),
        },
        Scenario {
            form: Form::PdfPage2,
            degradation: scan(20),
        },
    ]
}

/// Le plan du corpus : chaque gabarit décliné sur chaque scénario. Séparé de la
/// construction, qui rasterise et coûte cher.
pub fn plan(limit: Option<usize>) -> Vec<(usize, &'static layout::Layout, Scenario)> {
    let scenarios = scenarios();
    let total = layout::LAYOUTS.len() * scenarios.len();
    let wanted = limit.unwrap_or(total).min(total);

    (0..wanted)
        .map(|index| {
            (
                index,
                &layout::LAYOUTS[index % layout::LAYOUTS.len()],
                scenarios[index / layout::LAYOUTS.len()].clone(),
            )
        })
        .collect()
}

/// Produit le corpus. À lancer en `--release` : la rastérisation et les dégradations
/// sont dix fois plus lentes en profil de développement.
pub fn generate(seed: u64, limit: Option<usize>) -> Vec<Sample> {
    let mut rng = Rng::new(seed);

    plan(limit)
        .into_iter()
        .map(|(index, layout, scenario)| {
            let data = data::generate(&mut rng);

            package::build(
                index,
                layout,
                data,
                scenario.form,
                &scenario.degradation,
                &mut rng,
            )
        })
        .collect()
}

fn csv_field(value: &str) -> String {
    value.replace(';', ",").replace('\n', "|")
}

/// Écrit le corpus et sa vérité terrain, et renvoie le nombre d'échantillons produits.
pub fn write(dir: &Path, seed: u64, limit: Option<usize>) -> std::io::Result<usize> {
    fs::create_dir_all(dir)?;

    let samples = generate(seed, limit);
    let mut truth = String::from("file;iban;bic;account_holder;src;recipe;expect\n");

    for sample in &samples {
        fs::write(dir.join(&sample.name), &sample.bytes)?;

        truth.push_str(&format!(
            "{};{};{};{};{};{};{}\n",
            csv_field(&sample.name),
            sample.data.iban,
            sample.data.bank.bic,
            csv_field(&sample.data.holder()),
            sample.form.tag(),
            csv_field(&sample.recipe),
            if sample.form.known_failure() {
                "known_failure"
            } else {
                "ok"
            }
        ));
    }

    fs::write(dir.join("truth.csv"), truth)?;

    Ok(samples.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_covers_every_layout_and_scenario() {
        let plan = plan(None);

        assert_eq!(plan.len(), layout::LAYOUTS.len() * scenarios().len());

        for scenario in scenarios() {
            assert!(
                plan.iter().any(|(_, _, s)| s.form == scenario.form),
                "forme absente du corpus : {}",
                scenario.form.tag()
            );
        }

        for layout in layout::LAYOUTS.iter() {
            assert!(
                plan.iter().any(|(_, l, _)| l.name == layout.name),
                "gabarit absent du corpus : {}",
                layout.name
            );
        }
    }

    #[test]
    fn names_are_unique_and_carry_the_recipe() {
        let samples = generate(5, Some(6));
        let mut names: Vec<&str> = samples.iter().map(|s| s.name.as_str()).collect();

        names.sort_unstable();
        let count = names.len();
        names.dedup();

        assert_eq!(names.len(), count);
        assert!(samples.iter().any(|s| s.name.contains("natif")));
    }

    /// La vérité terrain ne doit jamais casser le format à cause d'un séparateur
    /// présent dans une donnée.
    #[test]
    fn truth_fields_survive_separators() {
        assert_eq!(csv_field("A;B"), "A,B");
        assert_eq!(csv_field("ligne1\nligne2"), "ligne1|ligne2");
    }
}
