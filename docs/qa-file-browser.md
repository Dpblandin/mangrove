# QA native de la consultation des fichiers

> Rapport historique de l'interface Vue/Tauri, dont les sources ont été retirées. Les parcours, tests et mesures ci-dessous décrivent le build de l'époque. La QA de l'application GPUI actuelle est documentée dans le [README natif](../src-gpui/README.md).

Validation effectuée le 30 septembre 2026 sur macOS ARM64, avec le bundle Tauri release et une fixture entièrement synthétique de **10 007 commits**, dont un commit présentant 46 changements mixtes. Le bundle de QA utilise un identifiant distinct de l’application habituelle. Le processus a été fermé puis relancé après la compilation finale.

## Parcours vérifiés

- **Diff et fichier complet** : ouverture centrale depuis la liste des fichiers modifiés ; le contenu conserve le chemin et le SHA sélectionnés. Les numéros de ligne et la coloration TypeScript apparaissent après le chargement du worker. Le passage depuis la vue compacte à la vue centrale ouvre la consultation du même fichier.
- **Historique et renommage** : `apps/backend/src/readers/CommitReader.ts` remonte à `apps/backend/src/LegacyCommitReader.ts`. Ouvrir une ancienne entrée affiche son ancien chemin et son SHA ; le retour à la révision initiale est proposé. Recliquer le même fichier dans la liste restaure la consultation initiale.
- **Pagination** : `src/history.ts` présente 40 changements par page. La deuxième page commence à un autre commit ; le retour arrière restaure le premier commit de la première page. Les contrôles restent utilisables dans le bundle natif.
- **Blame et clavier** : la liste annonce les commandes de navigation. La touche Fin sélectionne la dernière ligne ; Entrée ouvre le message, l’auteur, la date et le SHA du commit correspondant dans la consultation. Le chemin et le SHA du fichier restent ceux de la révision consultée.
- **Police** : le bouton d’agrandissement et le raccourci `⌘ +` portent le texte de 11 à 13 px. `⌘ 0` le restaure à 11 px dans la vue Fichier.
- **Binaire** : `assets/new-preview.bin` annonce l’absence d’affichage texte dans Fichier, sans présenter des octets comme du texte.
- **Vide** : `tests/fixtures/empty.txt` affiche explicitement « Fichier vide à cette révision ».
- **Suppression** : `packages/legacy/src/ObsoleteAdapter1.ts` reste absent au commit de suppression. Le bouton explicite vers le premier parent affiche ses deux lignes et change le SHA consulté.
- **Aperçu borné** : `src/generated/catalog.ts`, de 2 401 lignes, affiche un aperçu et annonce la limite de 2 000 lignes / 256 Kio. La consultation reste navigable.
- **Fin de ligne** : l’ancienne version de `notes/release.txt` affiche son contenu et l’avertissement d’absence de saut de ligne final.

## Dépôt source et contrôles automatisés

Un instantané de **113 entrées** du dépôt synthétique, y compris `.git`, compare les octets des fichiers, les cibles des liens symboliques et les dates de modification en nanosecondes avant et après la QA finale. Résultat : **aucune différence**, aucune entrée ajoutée ou supprimée. Cet instantané reste local et n’est pas publié.

Les contrôles de la livraison initiale passent : 286 tests TypeScript dans 14 fichiers, 51 tests Rust (2 unitaires et 49 d’intégration), 8 tests du générateur de fixture, vérifications de format/lint/types, Clippy avec avertissements interdits et compilation du bundle macOS release. Les tests couvrent aussi les réponses tardives, les objets manquants, les merges, les identités de chemins et les bornes de lecture. Deux revues `refactor` puis une passe `simplify` ont été appliquées avant ces contrôles.

Les mesures du moteur sur cette fixture et sur un dépôt réel de plus de 12 000 commits sont publiées dans [performance.md](performance.md) et [performance-files.json](performance-files.json), uniquement sous forme agrégée.

## Portée et limites

La QA interactive porte sur macOS ; Linux et Windows nécessitent encore une vérification interactive sur leurs plateformes. Les bornes de lecture, le parcours sur le premier parent, les attributions de frontière et les noms littéraux incompatibles avec le blame libgit2 sont décrits dans [file-browser.md](file-browser.md). Les mesures de performance concernent le moteur, sans inclure l’IPC, le rendu ou la coloration du frontend.

## Corrections de la revue de PR

La revue du commit initial a signalé deux défauts reproduits avant correction :

- **Renommage sur un ancien dossier** : trois commits créent puis modifient `item/contents.txt`, avant de le déplacer vers `item`. Le test d’intégration échouait avec un changement « modified » au lieu de « renamed ». Après correction, les trois entrées suivent le bon chemin et les deux contenus historiques sont lisibles. La transition inverse, fichier remplacé par dossier, reste une suppression du fichier ; l’historique n’inclut pas les changements du dossier. Les snapshots du dépôt source restent identiques.
- **Défilement horizontal** : le vrai composant Vue est monté dans une zone de 494 × 300 px, avec 100 lignes dont la première comporte 300 caractères. Avant correction, un défilement vertical de 1 000 px faisait passer `scrollWidth` de 2 052 à 488 px et `scrollLeft` de 1 000 à 0. Après correction, la largeur reste à 2 052 px en Fichier et 2 292 px en Blame, avec `scrollLeft = 1 000` conservé. Huit variantes avec tabulations, caractères CJK, accents combinés et emoji, longue ligne en début ou en fin, police de 11 ou 20 px et les deux vues conservent également leur largeur et leur position. Seules 26 à 33 lignes visibles ou anticipées sont montées dans ces variantes.

Cette vérification de largeur utilise le navigateur collaboratif et le composant réel, sans mock du virtualiseur. Elle complète la QA native initiale ; elle ne constitue pas une nouvelle QA interactive sur Linux ou Windows. La largeur est mesurée par la même fonte et les mêmes colonnes que les lignes visibles, sur le texte borné de l’aperçu.

Une vérification indépendante sur **2 000 lignes / 256 872 octets** maintient une largeur de 6 694 px et `scrollLeft = 1 000` après le défilement vertical, avec 33 lignes montées et la plus longue démontée. Remplacer le contenu par un fichier court réduit correctement la largeur. Les contrôles de la correction passent : **286 tests TypeScript**, **53 tests Rust** (2 unitaires et 51 d’intégration), format/lint/types et Clippy avec avertissements interdits.
