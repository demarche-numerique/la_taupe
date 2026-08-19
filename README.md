# La Taupe pour vous servir

Elle laboure vos documents pour en sortir des pépites.

## 2D-Doc

La documentation de référence sur les 2D-Doc se trouve sur le site de l'[ANTS](https://ants.gouv.fr/nos-missions/les-solutions-numeriques/2d-doc).
L'exemple justificatif_de_domicile.png est issu de leur "Spécifications Techniques des Codes à Barres 2D-DOC".

## Installation

Deux moteurs OCR sont disponibles ; PP-OCR est le moteur par défaut.

**PP-OCR** (PaddleOCR v6 tiny, via [`oar-ocr`](https://crates.io/crates/oar-ocr) et
ONNX Runtime, CPU seul). Mesuré sur les deux corpus réels contre ocrs : meilleur sur
tous les champs, deux fois moins de titulaires faux sur les photos, médiane divisée par
3,4.

Le binaire est **autonome** : les modèles (6 Mo) sont téléchargés au build par
`./download-models.sh` et embarqués par `include_bytes!`, ONNX Runtime est lié
statiquement. Rien n'est téléchargé à l'exécution — une prod hors ligne n'a rien à
pré-charger. Seule la **machine de build** a besoin du réseau, une fois : les modèles
vont dans `models/`, l'archive ONNX Runtime (~90 Mo) dans `~/.cache/ort.pyke.io/`, et
les deux sont réutilisés aux builds suivants. Pour un build sans réseau du tout, `ort`
lit `ORT_LIB_LOCATION` pointant sur une archive `libonnxruntime.a` fournie.

`LA_TAUPE_PPOCR_MODEL_DIR` pointe un répertoire `det.onnx` / `rec.onnx` / `dict.txt`
pour essayer un autre jeu de modèles sans rebuild. Sur les corpus réels, v6 small lit
deux BIC de plus sur les photos mais perd sur le titulaire et coûte le double ; v5
server met quarante secondes la page en CPU.

**tesseract** (`tesseract-ocr-fra`, dépendance système) sert de second avis sur les
recadrages que le moteur principal ne lit pas. Il est irremplaçable : mesuré sans lui,
l'IBAN des photos perd trois documents sur trente-deux, et un second modèle PP-OCR à sa
place n'en récupère qu'un — deux tailles d'un même modèle partagent leurs erreurs, la
redondance qui paie vient d'un moteur d'une autre famille.

Rust 1.95 minimum (`rust-toolchain.toml`), exigé par `oar-ocr`.
## Banc de mesure

Le pipeline de reconnaissance de RIB enchaîne plusieurs stratégies (texte du PDF,
OCR de la page, recadrage autour d'une ancre, redressement puis tesseract). Aucune
amélioration n'est évaluable sans mesure : le banc sert à comparer avant/après sur un
corpus, et à savoir quelle branche produit réellement les succès.

Il est bâti pour qu'un corpus de documents personnels puisse être mesuré **sans que son
contenu ne figure dans le rapport**. Celui-ci ne porte que des verdicts et des grandeurs
géométriques ; la garantie tient au typage de `bench::report::FileReport`, qui n'accepte
aucun texte issu de la reconnaissance.

### Corpus synthétique

    cargo run --release --features bench --bin synth -- --out <dir> [--seed N] [--count N]

Produit des RIB fictifs — IBAN structurellement valides (mod-97 et clé RIB) mais tirés
au hasard — déclinés sur cinq gabarits, trois formes d'entrée (PDF natif, PDF scanné,
photo) et une grille de dégradations paramétrées : résolution, inclinaison, perspective,
flou, bruit, éclairage inégal, compression. La vérité terrain est écrite en même temps.

La composition de la grille suit ce que le service reçoit réellement, établi sur deux
corpus : des photos pour l'essentiel, des PDF issus des chaînes éditiques bancaires, et
peu de scans. Ce sont les photos, non les scans, qui arrivent pivotées, et une photo
porte toujours un support visible et un éclairage inégal — il n'existe pas de « photo
propre ». Les PDF natifs restent peu nombreux malgré leur poids réel : reconnus sans
OCR, donc toujours à 100 %, les sur-représenter diluerait les écarts que le banc doit
rendre visibles. Le taux global n'est pas une estimation du taux de production, c'est un
indicateur de régression.

Chaque nom de fichier porte sa recette (`012_lcl_pdf_img_h14_rot30.pdf`), ce qui permet
au banc de rendre des courbes plutôt qu'un taux global : à quelle hauteur de capitale, à
quel angle, à quel niveau de bruit la reconnaissance cède.

### Mesure

    cargo run --release --features bench --bin bench -- \
      --corpus <dir> [--truth <csv>] [--confusion] [--jobs N]

La vérité terrain est un CSV `file;iban;bic;account_holder`, les lignes du titulaire
séparées par `|`, les colonnes repérées par leur nom. Un champ vide sort du calcul du
taux au lieu de compter comme un échec. Les colonnes facultatives `src`, `recipe` et
`expect` ventilent les résultats ; `expect=known_failure` isole les cas connus pour
échouer sans les compter comme des régressions.

`--bootstrap` amorce ce fichier en ne retenant que les IBAN qui valident à la fois le
mod-97 et la clé RIB, pour ne pas figer les erreurs du pipeline en référence.

`--check` contrôle la forme du fichier sans rien mesurer : IBAN mal recopié (le mod-97
le voit), clé RIB incohérente, BIC de forme inattendue, séparateur `|` oublié entre le
nom et son adresse, ligne désignant un document absent du corpus. À lancer avant toute
campagne de saisie — mesurer contre une référence fausse fait échouer le pipeline sur
des lectures pourtant correctes.

`--confusion` ajoute la matrice de confusion caractère, agrégée sur tout le corpus, et
la distribution des positions d'erreur — émises séparément, jamais jointes.


### Non-régression

`tests/bench_synth.rs` vérifie en intégration continue que les PDF natifs sont reconnus
intégralement et que le rapport ne laisse rien fuiter. La mesure complète, OCR compris,
demande tesseract et les modèles PP-OCR :

    cargo test --release --features bench --test bench_synth -- --ignored --nocapture
