# Mangrove — application GPUI

Application native de Mangrove, construite avec GPUI et le moteur partagé `mangrove-core`. La version Vue/Tauri est conservée pour la comparaison décrite dans [l’issue #9](https://github.com/Dpblandin/mangrove/issues/9). Ce dossier possède son propre workspace Cargo, son lockfile et son répertoire `target`, afin de garder la famille de dépendances GPUI cohérente sans modifier celle de Tauri.

Le portage couvre les vues et interactions de l’application existante. Les couleurs, espacements, tailles, icônes et tracés du graphe reprennent leurs références dans `src/style.css` et les composants Vue. Cela ne constitue pas une garantie d’identité pixel par pixel : le rendu du texte, certains contrôles natifs et la coloration syntaxique peuvent différer. Les mesures de performance doivent être interprétées avec les limites du [protocole de comparaison](../docs/performance-gpui-protocol.md).

## Lancer

Depuis la racine du dépôt, avec Rust et les prérequis GPUI de l’OS, dont [Xcode complet sur macOS](https://github.com/zed-industries/zed/blob/main/docs/src/development/macos.md) :

```sh
# Écran d’accueil, sans dépôt présélectionné.
cargo run --release --locked --manifest-path src-gpui/Cargo.toml

# Ouverture directe d’un dépôt local.
cargo run --release --locked --manifest-path src-gpui/Cargo.toml -- /chemin/du/depot

# Fixture synthétique utilisée pour la QA et les benchmarks.
vp run fixture --commits 10000 --large-change
cargo run --release --locked --manifest-path src-gpui/Cargo.toml -- .mangrove-dev/fixture-10000
```

La configuration actuelle de GPUI Kit active `runtime_shaders` : les shaders Metal sont compilés à l’exécution. La CI vérifie Xcode complet avec `xcode-select -p` et `xcodebuild -version` ; le composant optionnel Metal Toolchain n’est pas un prérequis de ce build.

Le chemin du dépôt est le seul argument. Le bouton d’ouverture et Cmd/Ctrl + O ouvrent directement le sélecteur natif de dossier. L’accueil permet aussi de rouvrir un dépôt récent. Fermer la dernière fenêtre ou utiliser Cmd/Ctrl + Q quitte l’application.

Le cache GPUI est distinct du cache Tauri, dans le répertoire de cache utilisateur de l’OS sous `dev.mangrove.gpui`. Les préférences sont enregistrées dans le répertoire de configuration utilisateur sous `mangrove-gpui/preferences.json` : thème, polices, largeurs des panneaux, intervalle d’auto-fetch et dépôts récents. `MANGROVE_GPUI_PREFERENCES=/chemin/absolu/preferences.json` permet de choisir un autre fichier, notamment pour une session de QA isolée. Les préférences Vue ne sont pas importées automatiquement.

Comme dans Vue, le fetch manuel récupère les références de tous les remotes du dépôt actif. L’auto-fetch utilise un intervalle de 0 à 60 minutes, avec une minute par défaut ; 0 le désactive. Les objets LFS ne sont pas téléchargés automatiquement. Le fetch peut mettre à jour les références distantes ; les vues de changements locaux ne proposent pas de staging, de commit ou de modification des fichiers.

## Périmètre porté

- Accueil, sélecteur natif de dossier, recherche/suppression des dépôts récents, onglets de dépôts, barre supérieure, erreurs et paramètres d’auto-fetch.
- Trois thèmes Classique, Mangrove et Macchiato, zoom de l’interface, taille de police des diffs, panneaux redimensionnables à la souris et au clavier, remise à zéro par double clic et préférences persistantes.
- Références locales, distantes et tags avec dossiers repliables, filtre, tri français et numérique, sélection dans l’historique et Solo. Worktrees et stashes sont lus à la demande ; un worktree peut être ouvert et un stash inspecté.
- Historique virtualisé, recherche par message/auteur/SHA/référence, navigation au clavier, graphe de branches et merges avec les mêmes voies du moteur et défilement horizontal du graphe.
- Inspection des commits, métadonnées et parents, copie du SHA, liste des fichiers, compteurs, changements locaux staged/unstaged et groupes de fichiers des stashes.
- Diff unifié central ou dans l’inspecteur, numéros de lignes, fonds d’ajout/suppression, contenu des fichiers, historique suivant les renommages et blame. Les vues de code disposent de défilement horizontal/vertical, de sélection et de copie du texte sans les gouttières.
- Lectures Git en arrière-plan, rejet des réponses obsolètes après une nouvelle sélection, listes virtualisées et caches bornés pour l’historique, les diffs et la coloration.

La coloration native utilise Tree-sitter et un fallback lexical lorsque la grammaire ou sa requête est indisponible, notamment pour CMake et Kotlin avec les dépendances actuelles. Vue utilise Shiki : la reconnaissance des tokens n’est donc pas identique, même avec les mêmes couleurs de thème. Les budgets de coloration et les limites du moteur peuvent produire un affichage en texte simple ou un patch tronqué ; le protocole contrôle cet état dans les scénarios comparés.

## Packaging macOS

Le script construit une application autonome locale avec l’icône de Mangrove et l’identifiant `dev.mangrove.gpui` :

```sh
sh src-gpui/scripts/package-macos.sh
open "src-gpui/target/release/bundle/Mangrove.app"
```

Le bundle n’est pas signé ni notarisé. La distribution, la signature et la notarisation restent à préparer ; les builds et le packaging Windows/Linux restent à valider sur ces plateformes.

## Vérification et limites de QA

```sh
cargo fmt --manifest-path src-gpui/Cargo.toml --check
cargo clippy --locked --manifest-path src-gpui/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-gpui/Cargo.toml
```

Les tests protègent notamment les caches bornés, la progression de l’index, les graphes, les filtres et la coloration, ainsi que les bornes et la persistance des préférences. Les tests de `mangrove-core` se lancent depuis le workspace principal.

La QA interactive réalisée les 30 septembre et 1er octobre 2026 porte sur macOS ARM, avec les fixtures synthétiques et un binaire release. Elle couvre l’historique, les diffs, le contenu/historique/blame des fichiers, les thèmes, les menus de copie/sélection, les panneaux et leurs remises à zéro. Les captures du portage sont dans [screenshots](screenshots/), notamment `full-history.jpg`, `full-diff.jpg`, `full-welcome.jpg`, `full-mangrove.jpg`, `full-macchiato.jpg` et `full-blame.jpg`. Le manifeste des captures identifie le binaire production vérifié et les fichiers correspondants. Les captures Vue/Tauri sont réalisées sur le bundle de mesure final en mode d’interface normal et identifiées séparément. La configuration normale passe 77 tests ; la configuration instrumentée en passe 82.

La dernière passe vérifie aussi les corrections signalées pendant la comparaison :

- Accueil : bouton principal accentué, champ de recherche bordé, récents et ouverture directe du sélecteur natif depuis le bouton.
- Ouverture native depuis la barre, le menu et Cmd/Ctrl + O : annulation sans perdre le dépôt consulté, ouverture immédiate du dossier choisi et erreur visible pour un dossier sans Git.
- Paramètres sur toute la fenêtre, sans onglets ni barres du workspace : styles du menu et des boutons, champ sombre avec flèches bornées à 0–60 et retour des fractions à une minute entière, validation, enregistrement, annulation et réinitialisation du brouillon à la réouverture. Retour et Escape retrouvent le workspace ; Cmd/Ctrl + K affiche sa recherche. Le bouton, le menu et le raccourci partagent la garde qui interdit l’ouverture pendant un redimensionnement actif ou un sélecteur natif. Capture : [post-comparison-settings.jpg](screenshots/post-comparison-settings.jpg).
- Aperçu volumineux sans lignes affichées : avertissement sous les onglets, avec texte lisible dans un panneau étroit. Capture : [post-comparison-large-file.jpg](screenshots/post-comparison-large-file.jpg).
- Barres de défilement visibles lorsqu’il existe un débordement, avec accès au dernier des 10 007 commits, aux derniers fichiers et à la fin du grand diff. Le défilement horizontal du code est vérifié après réduction de la largeur du panneau.
- Molette après déplacement lointain de la poignée : dans le code, passage de la ligne 1 488 à 1 537 ; dans la sidebar de 10 000 références, passage de `qa-05026` à `qa-05055`, puis retour à `qa-05026` en sens inverse. Les tests couvrent également les hauteurs aux échelles physiques 1×/2× et le zoom.
- Menu d’historique avec 51 références : hauteur bornée à la fenêtre, barre visible et sélection révélée avec Home/End. Blame rouvert après une inspection de commit sans perdre son contenu.
- Icône originale incluse dans le bundle production et reconnue par macOS via `NSRunningApplication`.
- Dépôt Mangrove réel sur `main`, vérifié propre avec Git : aucune ligne « Modifications non commitées ». Dans un clone jetable, sélectionner le diff local puis créer un commit externe et actualiser retire l’ancien diff et sélectionne le nouveau premier commit, même après une page zéro retardée.

Ces vérifications ne remplacent pas une comparaison visuelle exhaustive des états, une validation Windows/Linux ou un audit complet d’accessibilité. Elles ne démontrent pas à elles seules un gain de performance.

## Comparer les performances

Le build de mesure est distinct du build courant : `sh src-gpui/scripts/package-macos.sh --benchmark` produit `src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app`. La fonctionnalité `app-benchmark` doit être activée explicitement ; seules les variables de la procédure de mesure y déclenchent le scénario.

Le [protocole](../docs/performance-gpui-protocol.md) décrit la compilation des deux applications, les commandes du benchmark, les répétitions, les caches isolés et les mesures de démarrage, sélection, coloration, défilement, CPU et mémoire. Les préférences du benchmark GPUI utilisent un fichier temporaire ; les deux interfaces affichent le même dépôt, la même fenêtre et les mêmes valeurs par défaut. Les résultats et leur contexte sont consignés dans [performance-gpui.md](../docs/performance-gpui.md) après la série de mesures.

Les mesures de rendu utilisent des occasions de rendu WebView et des soumissions de frames GPUI, avec leurs limites documentées ; elles ne mesurent pas physiquement le délai clic→pixels de l’écran. Le coût de Git, de l’indexation et du réseau reste celui du moteur partagé. Les benchmarks du moteur seul dans `docs/performance.md` ne mesurent pas le gain d’une migration de l’interface.

## Dépendances et API

L’application dépend de `gpui-kit = 0.7.0`, avec la fonctionnalité `tree-sitter-languages`. La fonctionnalité Cargo `app-benchmark` active le profiler et les scénarios de mesure ; le build normal ne contient pas ce lanceur de benchmark et ignore les variables `MANGROVE_BENCH_*`. Cette version sélectionne sa famille cohérente `gpui-pre-* = 0.3.7` ; ne pas ajouter indépendamment une version du crate officiel `gpui`. GPUI reste pré-1.0 et ses API peuvent évoluer.

Sources vérifiées le 30 septembre 2026 : [GPUI](https://gpui.rs/), [GPUI Kit](https://gpui-kit.com/), [GPUI Base](https://gpui-kit.com/base/), [mesure des frames](https://gpui-kit.com/docs/fps/) et [README GPUI / stabilité de l’API](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md).
