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
l'IBAN des photos perd neuf points, et un second modèle PP-OCR à sa
place n'en récupère que trois — deux tailles d'un même modèle partagent leurs erreurs, la
redondance qui paie vient d'un moteur d'une autre famille.

Rust 1.95 minimum (`rust-toolchain.toml`), exigé par `oar-ocr`.
## Référentiels embarqués

Deux fichiers de référence sont compilés dans le binaire (`include_str!`) :

- `src/riad_bank_name.csv` — code banque → BIC, nom ; extrait de la liste des IFM de la
  BCE. Donne le nom de la banque et valide le code établissement du BIC contre le code
  banque de l'IBAN.
- `src/code_postal_commune.csv` — code postal → libellé d'acheminement ; extrait de la
  Base officielle des codes postaux de La Poste (Licence Ouverte 2.0). Corrige la ligne
  de ville d'un titulaire quand le code postal est sûr et la ville approchante.

`./scripts/update-references.sh` les rafraîchit tous les deux : il trouve la dernière
édition BCE, fusionne en gardant le BIC précédent quand la nouvelle édition l'a vidé
(la BCE cesse d'en maintenir certains d'une édition à l'autre), et affiche ce qui change
avant de remplacer. À lancer de temps en temps, puis `git diff` et commit.

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

## Pistes mesurées et écartées

Chaque entrée a été implémentée, mesurée sur les deux corpus réels (des photos d'un
côté, des PDF et images de l'autre), puis retirée. Le code reste sur les branches `bench_rib` et `oar_ocr` pour
qui veut rejouer. Sauf mention contraire, « neutre » signifie : aucun verdict ne
change, à la marge de bruit près — une dizaine de points, c'est ce que deux
toolchains font bouger sur le même code.

**Redressement des documents pivotés d'un quart de tour.** Deux détecteurs — une
vignette reconnue dans les quatre sens, puis le classifieur natif de PP-OCR.
Spectaculaires en synthétique (documents pivotés 60 → 100 %), neutres à négatifs en
réel : les documents pivotés sont déjà lus sans eux, et faire basculer un document de
la branche tesseract vers ocrs accélère l'IBAN au prix du BIC et du titulaire. Le cas
où le gain apparaît — un scan propre pivoté d'un quart de tour exact — n'existe dans
aucun des deux corpus.

**Projection du texte OCR en grille, pour appliquer le chemin texte aux images.** La
projection est fidèle, mais `text::patch::complete` coupe un bloc au premier
double-espace, convention `pdftotext` que l'OCR ne tient pas : le titulaire des
photos tombe de 62 à 9 %. La suite, pour qui reprend : ne produire un double-espace
qu'à un écart mesuré en largeurs de caractère.

**Lire le titulaire dans les lignes de la page au lieu de recadrer.** −58 % d'appels
OCR, mais titulaire des photos 62 → 41 % : le recadrage est un vrai zoom, la
reconnaissance rapprochée lit les petits caractères que la passe pleine page rate.

**Réglages tesseract sur les recadrages d'IBAN** (`--psm 7`, sans dictionnaires) et
**alphabet restreint pour ocrs** : `psm 7` perd six points d'IBAN (un recadrage fait
cinq hauteurs de ligne et en contient parfois plusieurs), le reste est neutre.

**Mise à jour de rten 0.22 → 0.25** : +6 % de latence à appels constants, et exigerait
Rust 1.94. Sans objet depuis le retrait d'ocrs.

**Garder ocrs en second moteur sélectionnable.** Mesuré une dernière fois avant retrait :
en agrégat il perd sur tous les champs des deux corpus — IBAN, BIC et titulaire, avec
une latence 3,4 fois plus longue — et la garde « illisible » élimine un document sur
dix, qu'il lit pourtant. Il ne gagnait plus que quelques titulaires isolés — cinq
points en tout — pas de quoi payer 12 Mo de modèles morts dans le binaire et une
seconde pile de reconnaissance.
Retiré ; ses types d'échange (`TextLine`, mots positionnés) survivent dans `src/lines.rs`.

**Un second modèle PP-OCR en repli de tesseract** (v6 small) : récupère un des trois
IBAN que tesseract récupère, et le titulaire tombe de 62 à 47 %. Deux tailles d'un
même modèle partagent leurs erreurs.

**Classification d'orientation native de PP-OCR** : qualité identique, +26 % de
latence.

**PP-OCR v5 server** : 42 secondes la page en CPU.

**Ancrer le titulaire sur la civilité** (« M », « MME », « Monsieur »… en tête de
ligne), quand ni code postal ni libellé « titulaire » ne le localisent. Le repère est
juste : sur les trois photos réelles sans titulaire, il trouve le bloc à chaque fois.
Mais aucun des trois n'est exact — un nom à un ou deux caractères près, une adresse
fausse sous un nom juste, un bloc incomplet — et le taux de titulaires justes ne bouge
pas. Retiré, par choix : **ne rien rendre plutôt qu'un bloc douteux**, comme pour le
BIC. À reprendre si un référentiel (communes, voies) permet de valider le bloc trouvé.

**Localiser le bloc titulaire par NER zéro-shot** (GLiNER, `gliner_multi_pii-v1`,
289 M paramètres, via gline-rs). Le modèle voit la ligne du nom sur 72 à 85 % des
documents, mais ne sait pas borner le bloc dans le texte linéarisé : sur le bloc
complet il fait moitié moins bien que la cascade, sur chaque corpus. En repli quand la
cascade ne rend rien, il rendrait plus de blocs faux que de justes. S'y ajoutent
1,1 Go de modèle (l'export int8 de 332 Mo ne rend aucune entité), 0,4 à 0,9 s par
document, et un conflit de version ONNX Runtime avec oar-ocr. L'évaluation complète
est rejouable dans `gitignored/gliner_eval/`.

## Ce qu'il faut savoir pour mesurer

- **Un travail à la fois** (`--jobs 1`). À quatre, les appels ONNX Runtime se disputent
  les cœurs et chaque appel coûte deux à trois fois plus : la mesure ne reflète pas
  la prod, qui traite une requête à la fois.
- **La marge de bruit est d'une dizaine de points par corpus.** Le même code compilé
  par deux toolchains lit un recadrage à un ou deux caractères près, assez pour
  faire basculer un mod-97. En deçà, un écart entre deux mesures ne dit rien.
- **Le corpus synthétique est calibré sur l'IBAN et le titulaire, et désormais sur le
  BIC.** Il a longtemps été faux sur le BIC à cause d'ocrs, pas du générateur. Il
  reste plus propre que le réel : un quart des photos réelles reconnues n'a aucun
  vocabulaire de RIB lisible, contre aucune en synthétique.
