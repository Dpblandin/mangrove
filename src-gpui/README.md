# Mangrove — application GPUI

Application native de Mangrove, construite avec GPUI et le moteur `mangrove-core`. Ce dossier possède son propre workspace Cargo, son lockfile et son répertoire `target` ; le workspace principal contient le moteur et ses benchmarks.

Les couleurs, arrondis et polices sont définis dans `src/theme.rs` ; les vues GPUI portent les espacements, contrôles et interactions. Les icônes originales de packaging sont dans `assets/icons`. La migration depuis l’ancienne interface Vue/Tauri et sa fidélité sont documentées dans le [rapport de comparaison](../docs/performance-gpui.md). Les sources de cette ancienne interface ont été retirées ; les résultats et captures de la comparaison restent historiques, avec les limites de leur [protocole](../docs/performance-gpui-protocol.md).

## Lancer

Depuis la racine du dépôt, avec Rust et les prérequis GPUI de l’OS, dont [Xcode complet sur macOS](https://github.com/zed-industries/zed/blob/main/docs/src/development/macos.md) :

```sh
# Écran d’accueil, sans dépôt présélectionné.
cargo run --release --locked --manifest-path src-gpui/Cargo.toml

# Ouverture directe d’un dépôt local.
cargo run --release --locked --manifest-path src-gpui/Cargo.toml -- /chemin/du/depot

# Fixture synthétique utilisée pour la QA et les benchmarks.
node scripts/create-fixture.mjs --commits 10000 --large-change
cargo run --release --locked --manifest-path src-gpui/Cargo.toml -- .mangrove-dev/fixture-10000
```

Le générateur de fixture utilise Node.js et Git, sans dépendance npm à installer. L’application elle-même utilise seulement son binaire natif. Pour compiler sans lancer :

```sh
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
```

La configuration actuelle de GPUI Kit active `runtime_shaders` : les shaders Metal sont compilés à l’exécution. La CI vérifie Xcode complet avec `xcode-select -p` et `xcodebuild -version` ; le composant optionnel Metal Toolchain n’est pas un prérequis de ce build.

Le chemin du dépôt est le seul argument. Le bouton d’ouverture et Cmd/Ctrl + O ouvrent directement le sélecteur natif de dossier. L’accueil permet aussi de rouvrir un dépôt récent ou un favori local du profil. Fermer la dernière fenêtre ou utiliser Cmd/Ctrl + Q quitte l’application.

Le cache est placé dans le répertoire de cache utilisateur de l’OS sous `dev.mangrove.gpui`. Les préférences sont enregistrées dans le répertoire de configuration utilisateur sous `mangrove-gpui/preferences.json` : thème, polices, largeurs des panneaux, intervalle d’auto-fetch, disposition des diffs et dépôts récents. `MANGROVE_GPUI_PREFERENCES=/chemin/absolu/preferences.json` permet de choisir un autre fichier, notamment pour une session de QA isolée.

Les rubriques **Profils** et **Intégrations** isolent comptes, favoris, préférences et récents, avec des lectures GitLab/GitHub/Bitbucket en arrière-plan. Les secrets utilisent le coffre de l’OS. Le choix SSH sert à consulter explicitement les branches et tags distants sans télécharger d’objets. Voir [la configuration, les éditions et limites de validation](../docs/integrations.md). `MANGROVE_PROFILES_PATH=/chemin/absolu/profiles.json` permet d’isoler les métadonnées avec le fichier de préférences de QA.

Le fetch manuel récupère les références de tous les remotes du dépôt actif. L’auto-fetch utilise un intervalle de 0 à 60 minutes, avec une minute par défaut ; 0 le désactive. Les objets LFS ne sont pas téléchargés automatiquement. Après des échecs successifs, l’auto-fetch espace ses tentatives jusqu’à 15 minutes ; une erreur fermée ne revient pas tant qu’elle ne change pas. Le fetch peut mettre à jour les références distantes ; les vues de changements locaux ne proposent pas de staging, de commit ou de modification des fichiers.

## Fonctionnalités

- Accueil, sélecteur natif de dossier, recherche/suppression des dépôts récents, onglets de dépôts, barre supérieure, erreurs et paramètres d’auto-fetch. Les worktrees récents sont regroupés sous leur dépôt principal avec un retrait ; rechercher une branche conserve son dépôt comme contexte. Le principal reste accessible même s’il n’est plus dans les six entrées récentes.
- Six thèmes Classique, Mangrove, Macchiato, Ardoise, GitLab et Octopus dans **Paramètres → Général → Apparence**, appliqués et mémorisés immédiatement pour le profil actif ; taille du texte de l’interface et des diffs, réglables au même endroit ou par raccourci, panneaux redimensionnables à la souris et au clavier, remise à zéro par double clic ou Entrée sur le séparateur et préférences persistantes. GitLab reprend la palette sombre de GitLab, ses couleurs de statuts et de diffs ainsi que sa coloration syntaxique. Octopus, anthracite et bleu, utilise dix couleurs de branches dans le graphe.
- Palette globale **Cmd/Ctrl + P**, également dans le menu **Affichage → Palette de commandes…** : recherche sans distinction de casse ni d’accents, raccourcis indiqués, choix des thèmes et commandes contextuelles. **↑ / ↓** sélectionnent, **Entrée** exécute, **Échap** ferme et restitue le focus. [Inventaire des commandes](../docs/command-palette.md).
- Branches locales, branches distantes et tags avec dossiers repliables, filtre, tri français et numérique, sélection dans l’historique et Solo. Une branche locale suivie affiche son avance et son retard sur l’upstream (`↑3 ↓12`, « disparu »), repris dans la puce HEAD de l’historique. Worktrees et stashes conservent leurs sections dédiées et sont lus à la demande ; un worktree peut être ouvert et un stash inspecté. Au clavier, **↓** depuis le filtre entre dans l’arbre ; **↑ / ↓** parcourent les lignes visibles, **← / →** replient, déplient ou passent au parent et au premier enfant, **Début / Fin** vont aux extrémités, **Entrée** agit comme un clic et **Échap** revient au filtre.
- Historique virtualisé, recherche par message/auteur/SHA/référence avec surlignage, navigation au clavier, menu contextuel de copie du SHA et du message, graphe de branches et merges avec les mêmes voies du moteur et défilement horizontal du graphe.
- Inspection des commits, métadonnées et parents, copie du SHA, liste des fichiers, compteurs, changements locaux staged/unstaged et groupes de fichiers des stashes.
- Diff central unifié ou côte à côte, diff unifié dans l’inspecteur, numéros de lignes fixes pendant le défilement horizontal, navigation entre blocs, fonds d’ajout/suppression, mots modifiés surlignés dans les lignes appariées, contenu des fichiers, historique suivant les renommages et blame. Les vues de code disposent de défilement horizontal/vertical, de sélection et de copie du texte sans les gouttières.
- Aperçus PNG, JPEG, GIF, WebP, BMP, ICO et TIFF dans les vues Diff et Fichier ; comparaison avant/après pour les commits, changements locaux et stashes, y compris en vue compacte. **Paramètres → Général → Aperçus des fichiers** permet de les désactiver ; le réglage est actif par défaut et enregistré immédiatement. La même carte règle la taille maximale des fichiers lus pour un diff ou un contenu : 8 Mio par défaut, de 2 à 64 Mio. Au-delà, la vue indique la taille du fichier et **Afficher quand même** le lit jusqu’à 64 Mio. Les limites sont détaillées dans [la consultation des fichiers](../docs/file-browser.md).
- Lectures Git en arrière-plan, rejet des réponses obsolètes après une nouvelle sélection, listes virtualisées et caches bornés pour l’historique, les diffs et la coloration.

La coloration native utilise Tree-sitter et un fallback lexical lorsque la grammaire ou sa requête est indisponible, notamment pour CMake et Kotlin avec les dépendances actuelles. Les budgets de coloration et les limites du moteur peuvent produire un affichage en texte simple ou un patch tronqué ; voir [l’architecture](../docs/architecture.md) et [la consultation des fichiers](../docs/file-browser.md).

## Packaging

Le script macOS construit une application autonome locale avec l’icône de Mangrove et l’identifiant `dev.mangrove.gpui` :

```sh
sh src-gpui/scripts/package-macos.sh
open "src-gpui/target/release/bundle/Mangrove.app"
# Build Intel depuis un Mac Apple Silicon, dans target/x86_64-apple-darwin/release/bundle :
sh src-gpui/scripts/package-macos.sh --target x86_64-apple-darwin
```

Le bundle est signé ad hoc, sans certificat Developer ID ni notarisation. Sous Linux, `sh src-gpui/scripts/package-linux.sh` prépare dans `target/release/bundle/mangrove` un préfixe `bin/` et `share/` avec l’entrée du lanceur et les icônes de `packaging/linux`. Sous Windows, `build.rs` intègre à l’exécutable l’icône déclarée dans `packaging/windows`, GPUI fournissant son manifeste DPI, et les builds release s’ouvrent sans console. `sh src-gpui/scripts/third-party-licenses.sh <fichier>` écrit les licences des composants tiers avec [cargo-about](https://github.com/EmbarkStudios/cargo-about) et la configuration de `packaging/about.toml`. Le [workflow de release](../docs/releases.md) assemble ces builds en DMG, archive `.tar.gz` et archive `.zip`, avec la licence de Mangrove et ces licences tierces. Les parcours interactifs restent à valider sous Windows et Linux.

## Vérification et limites de QA

```sh
cargo fmt --manifest-path src-gpui/Cargo.toml --check
cargo clippy --locked --manifest-path src-gpui/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-gpui/Cargo.toml
```

Les tests protègent notamment les caches bornés, la progression de l’index, les graphes, les filtres et la coloration, ainsi que les bornes et la persistance des préférences. Les tests de `mangrove-core` et du générateur se lancent depuis la racine :

```sh
cargo fmt --all --check
cargo clippy --locked -p mangrove-core --all-targets -- -D warnings
cargo test --locked -p mangrove-core
node --test scripts/create-fixture.test.mjs
```

La QA interactive réalisée les 30 septembre et 1er octobre 2026 porte sur macOS ARM, avec les fixtures synthétiques et un binaire release. Elle couvre l’historique, les diffs, le contenu/historique/blame des fichiers, les thèmes, les menus de copie/sélection, les panneaux et leurs remises à zéro. Les captures du portage sont dans [screenshots](screenshots/), notamment `full-history.jpg`, `full-diff.jpg`, `full-welcome.jpg`, `full-mangrove.jpg`, `full-macchiato.jpg` et `full-blame.jpg`. Le manifeste des captures identifie le binaire production vérifié et les fichiers correspondants. Les captures historiques Vue/Tauri sont identifiées séparément. La configuration normale passe 77 tests ; la configuration instrumentée en passe 82.

La dernière passe vérifie aussi les corrections signalées pendant la comparaison :

- Accueil : bouton principal accentué, champ de recherche bordé, récents et ouverture directe du sélecteur natif depuis le bouton.
- Ouverture native depuis la barre, le menu et Cmd/Ctrl + O : annulation sans perdre le dépôt consulté, ouverture immédiate du dossier choisi et erreur visible pour un dossier sans Git.
- Paramètres sur toute la fenêtre, sans onglets ni barres du workspace : styles du menu et des boutons, champ sombre avec flèches bornées à 0–60 et retour des fractions à une minute entière, enregistrement dès qu’une valeur valide est saisie, erreur en ligne et retour à l’intervalle enregistré quand le champ perd le focus. Échap quitte d’abord le champ, puis Retour ou un second Échap retrouvent le workspace ; Cmd/Ctrl + K affiche sa recherche. Le bouton, le menu et le raccourci partagent la garde qui interdit l’ouverture pendant un redimensionnement actif ou un sélecteur natif. Capture : [post-comparison-settings.jpg](screenshots/post-comparison-settings.jpg).
- Aperçu volumineux sans lignes affichées : avertissement sous les onglets, avec texte lisible dans un panneau étroit. Capture : [post-comparison-large-file.jpg](screenshots/post-comparison-large-file.jpg). Depuis, un fichier au-delà de la limite d’aperçu affiche sa taille, la limite et **Afficher quand même** au centre de la vue.
- Sélection d’un commit : aucun fichier n’est marqué avant un choix explicite. L’aperçu du premier fichier reste prêt en vue compacte ; les clics et le clavier sélectionnent le fichier choisi. Une actualisation conserve un choix explicite par sa clé, sans transformer l’aperçu initial ou un fichier de remplacement en sélection.
- Barres de défilement visibles lorsqu’il existe un débordement, avec accès au dernier des 10 007 commits, aux derniers fichiers et à la fin du grand diff. Le défilement horizontal du code est vérifié après réduction de la largeur du panneau.
- Molette après déplacement lointain de la poignée : dans le code, passage de la ligne 1 488 à 1 537 ; dans la sidebar de 10 000 références, passage de `qa-05026` à `qa-05055`, puis retour à `qa-05026` en sens inverse. Les tests couvrent également les hauteurs aux échelles physiques 1×/2× et le zoom.
- Clic sur un worktree : sur une fixture de 263 branches et 13 worktrees, la position de la sidebar reste identique après ouverture d’un worktree lié et retour au principal. Le filtre actif est conservé et le nouveau worktree est surligné. Le retour en haut est reproduit avec le binaire précédent, puis le parcours est vérifié dans le build debug corrigé avec des préférences isolées.
- Menu d’historique avec 51 références : hauteur bornée à la fenêtre, barre visible et sélection révélée avec Home/End. Blame rouvert après une inspection de commit sans perdre son contenu.
- Icône originale incluse dans le bundle production et reconnue par macOS via `NSRunningApplication`.
- Dépôt Mangrove réel sur `main`, vérifié propre avec Git : aucune ligne « Modifications non commitées ». Dans un clone jetable, sélectionner le diff local puis créer un commit externe et actualiser retire l’ancien diff et sélectionne le nouveau premier commit, même après une page zéro retardée.

Ces vérifications ne remplacent pas une comparaison visuelle exhaustive des états, une validation Windows/Linux ou un audit complet d’accessibilité. Elles ne démontrent pas à elles seules un gain de performance.

## Mesures de performance

Le build de mesure est distinct du build courant : `sh src-gpui/scripts/package-macos.sh --benchmark` produit `src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app`. La fonctionnalité `app-benchmark` doit être activée explicitement ; seules les variables de la procédure de mesure y déclenchent le scénario.

Le [protocole](../docs/performance-gpui-protocol.md) décrit les scénarios instrumentés, répétitions, caches isolés et mesures de démarrage, sélection, coloration, défilement, CPU et mémoire. Les préférences du benchmark GPUI utilisent un fichier temporaire. Le [rapport](../docs/performance-gpui.md) conserve les résultats de la comparaison avec l’ancien build Vue/Tauri ; cette référence ne fait plus partie des sources courantes.

Les soumissions de frames GPUI ne mesurent pas physiquement le délai clic→pixels de l’écran. La comparaison historique utilise aussi des occasions de rendu WebView, avec leurs limites documentées. Le coût de Git, de l’indexation et du réseau reste celui du moteur. Les [benchmarks du moteur seul](../docs/performance.md) excluent le rendu de l’interface.

## Dépendances et API

L’application dépend de `gpui-kit = 0.7.0`, avec la fonctionnalité `tree-sitter-languages`. La fonctionnalité Cargo `app-benchmark` active le profiler et les scénarios de mesure ; le build normal ne contient pas ce lanceur de benchmark et ignore les variables `MANGROVE_BENCH_*`. Cette version sélectionne sa famille cohérente `gpui-pre-* = 0.3.7` ; ne pas ajouter indépendamment une version du crate officiel `gpui`. GPUI reste pré-1.0 et ses API peuvent évoluer.

Sources vérifiées le 30 septembre 2026 : [GPUI](https://gpui.rs/), [GPUI Kit](https://gpui-kit.com/), [GPUI Base](https://gpui-kit.com/base/), [mesure des frames](https://gpui-kit.com/docs/fps/) et [README GPUI / stabilité de l’API](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md).
