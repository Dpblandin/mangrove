# Mangrove

Application **desktop** pour explorer Git. La consultation conserve les fichiers et l’index ; l’auto-fetch met à jour les références distantes. GPUI dessine l’interface native et utilise le moteur Rust `mangrove-core`. L’application autonome ne nécessite ni serveur local, ni navigateur, ni Node.js.

Le cahier des charges complet est dans [l’issue #1](https://github.com/Dpblandin/mangrove/issues/1).

## Fonctionnalités

- Ouverture de dépôts locaux, bare et worktrees ; dépôts récemment ouverts.
- Onglets de dépôts avec sélection, fermeture et bouton d’ouverture. Chaque changement recharge le dépôt actif et retrouve l’état de l’onglet : commit et fichier sélectionnés, diff central ouvert, recherche, Solo, replis et position de la sidebar. Un stash ou les modifications locales sélectionnés sont aussi retrouvés. L’accueil a toujours son onglet, y compris au lancement et après la fermeture du dernier onglet. Au clavier, **Tab** atteint les onglets et leur bouton de fermeture, **← / →** passent de l’un à l’autre et **Entrée** active ; **Ctrl + Tab** et **Ctrl + Maj + Tab** passent à l’onglet suivant ou précédent, **Cmd/Ctrl + 1…8** à l’onglet correspondant et **Cmd/Ctrl + 9** au dernier, aussi depuis le menu **Fenêtre**.
- Accueil centré : logo Mangrove, ouverture ou clonage d’un dépôt, favoris du profil et dépôts récents. Le panneau des branches n’apparaît qu’avec un dépôt.
- Historique topologique paginé, liste virtualisée et graphe Canvas.
- Branches locales, références distantes et tags ; dossiers imbriqués repliables et navigation vers leur commit. Le filtre révèle les dossiers des références correspondantes sans perdre les replis de la navigation normale.
- Mode **Solo** sur une ou plusieurs branches locales/distantes, accessible par clic droit ou bouton clavier : parcours et graphe recalculés, sélection du premier commit si le commit choisi en est absent, recherche limitée à ce parcours et retour explicite à la vue complète. La branche réellement active reste indiquée.
- Sections **Worktrees** et **Stashes** chargées à la demande ; ouverture des worktrees existants en onglets, états manquant/verrouillé/détaché, et consultation séparée des diffs sauvegardés du worktree, de l’index et des fichiers non suivis. [Comportements et limites](docs/repository-context.md).
- La navigation affiche uniquement les branches locales, les branches distantes et les tags. L’historique, le graphe et la recherche suivent ces branches, ces tags et HEAD. Les worktrees et les stashes conservent leurs sections dédiées.
- Recherche locale dans les messages, auteurs et emails, avec préfixes de mots ; recherche par préfixe de SHA ou de référence.
- Détails des commits, liste de fichiers virtualisée et diffs unifiés à la demande. Les merges sont comparés au premier parent. Les totaux +/− du commit sont lus en une passe interruptible après le premier diff et restent masqués au-delà de 1 000 fichiers ou si un fichier dépasse 2 Mio ; un renommage affiche « ancien → nouveau ».
- Consultation du contenu complet à la révision sélectionnée, historique paginé suivant les renommages détectés et blame à la demande ; chemin et SHA conservés entre les vues. Les groupes de blame ouvrent le commit concerné. Rendu virtualisé, coloration et police partagées avec les diffs ; [limites et comportements](docs/file-browser.md) documentés.
- Consultation des modifications non commitées : fichiers indexés, non indexés et non suivis, avec leurs diffs en lecture seule. Le statut est actualisé à l’ouverture du dépôt, au retour du focus dans l’application et par une actualisation manuelle. Aucun bouton ne permet de préparer un fichier pour un commit, de désindexer un fichier ou de créer un commit.
- Fichiers ajoutés en vert, supprimés en rouge, modifiés en jaune et renommés en bleu, dans tous les thèmes.
- Icônes de statut distinctes et chemins compacts dans les listes de fichiers. Les diffs gardent des fonds vert et rouge marqués, des numéros de ligne teintés sur les ajouts et suppressions et des en-têtes de bloc à la couleur d’accent, avec une coloration syntaxique préchargée progressivement pour le commit ou les modifications non commitées sélectionnés ; le texte ordinaire reste clair. Les fichiers indexés et non indexés sont préparés séparément et chaque actualisation invalide les anciens patches et couleurs.
- Le bouton « + » ouvre l’accueil dans un nouvel onglet. Les onglets rechargent le dépôt à l’activation ; le loader apparaît seulement si l’ouverture dépasse 500 ms.
- Un clic sur un fichier ouvre son diff dans la zone centrale, avec les détails et fichiers du commit à droite. La vue compacte reste disponible ; fermer le diff ramène à l’historique à la même position. Au clavier, **Tab** atteint la liste des fichiers et les boutons de vue ; **↑ / ↓** ouvrent le fichier voisin, **Entrée** ou **Espace** le fichier actif. Le mode Diff, Fichier, Historique ou Blame est conservé d’un fichier à l’autre lorsqu’il s’applique.
- La zone de lecture du message du commit s’ajuste à son contenu, entre deux lignes et environ 136 px, puis défile : un message long ne repousse pas la liste des fichiers au-delà de ce plafond.
- Aucun loader animé pendant les lectures de commits ou de diffs : le contenu précédent reste affiché, atténué et marqué « Chargement… », jusqu’à ce que la nouvelle sélection soit prête ; la copie du SHA est désactivée pendant ce temps. Une vue vide nomme le fichier en cours de lecture.
- Indexation progressive dans un cache SQLite FTS5 séparé du dépôt ; réutilisation à la prochaine ouverture.
- Cinq thèmes mémorisés dans **Paramètres → Général → Apparence** : **Classique** par défaut, gris/bleu ; **Mangrove**, vert ; **Catppuccin Macchiato** ; **Ardoise**, bleu ardoise pensé pour la lecture, avec un texte et des états lisibles à environ 7:1 sur les panneaux ; **Gitlab**, reprenant la palette sombre de GitLab, avec un fond presque noir, des panneaux gris légèrement violets, des liens bleu clair et ses couleurs de statuts, de diffs et de coloration syntaxique. Le choix est appliqué et enregistré immédiatement. Les arrondis sont communs à tous les thèmes.
- Paramètres dans une vue dédiée avec navigation par rubrique, accessibles par **Cmd + ,** sur macOS ou **Ctrl + ,** sur Windows/Linux. Dans un champ, **Échap** le quitte d’abord sans perdre sa valeur ; un second **Échap** ou le bouton de retour retrouve l’espace de travail précédent.
- Palette de commandes globale avec **Cmd + P** sur macOS ou **Ctrl + P** sur Windows/Linux : recherche des actions, navigation au clavier, ouverture des paramètres et choix direct du thème. Les commandes s’adaptent au dépôt et au fichier sélectionnés ; [couverture et fonctionnement](docs/command-palette.md).
- Auto-fetch des remotes du dépôt actif : dans **Paramètres → Général**, intervalle de **0 à 60 minutes**, enregistré après une courte pause de saisie, avec Entrée ou les flèches, ou en quittant le champ (une saisie invalide abandonnée rétablit la dernière valeur enregistrée), **1 minute** par défaut et **0** pour désactiver. L’historique et les références sont actualisés en conservant la recherche et la sélection. Un bouton dans la barre supérieure permet un fetch immédiat, même lorsque l’auto-fetch est désactivé. Les erreurs sont affichées sans interrompre les autres remotes, résumées en une ligne (réseau injoignable, hôte introuvable, authentification refusée, délai dépassé) avec le détail de Git dépliable et copiable. Une erreur fermée ne revient pas dans ce dépôt tant qu’elle ne change pas et qu’aucun fetch n’a réussi ; après des échecs successifs, l’intervalle double jusqu’à 15 minutes, puis revient à la normale après un succès. Le fetch manuel affiche toujours son résultat ; les objets LFS ne sont pas récupérés automatiquement.
- Fond sombre pour l’historique et les diffs ; Macchiato utilise le même fond que le panneau des détails.
- Panneaux branches et détails redimensionnables à la souris ou au clavier ; leurs largeurs sont mémorisées et les colonnes de l’historique s’adaptent à l’espace disponible. **Tab** atteint les séparateurs : **← / →** redimensionnent, avec **Maj** pour un pas plus grand ; **Entrée** ou un double clic réinitialisent.
- Taille des textes mémorisée : **Cmd/Ctrl + +** (ou **=**) agrandit, **Cmd/Ctrl + −** réduit, **Cmd/Ctrl + 0** rétablit la taille par défaut.
- Le texte des diffs démarre à **12 px**, comme le texte des listes de l’interface. Les boutons **− / +** dans leur en-tête règlent uniquement cette taille, mémorisée et partagée entre les vues centrale et compacte. Les raccourcis globaux ajustent les deux tailles ; **Cmd/Ctrl + 0** rétablit aussi le diff à 12 px. Les boutons **Bloc précédent / suivant** et **Alt + ↑ / ↓** (**⌥ + ↑ / ↓** sur macOS) passent d’un bloc du diff à l’autre ; les numéros de ligne restent visibles pendant le défilement horizontal.

Les paramètres proposent des profils personnel/professionnel et des comptes GitLab, GitHub et Bitbucket pour consulter les dépôts accessibles, PR/MR, discussions et résultats CI. GitLab et GitHub proposent une connexion dans le navigateur ; la configuration de leur application est repliée sous **Configuration avancée**. Les tokens sont conservés dans le coffre sécurisé de l’OS ; les préférences, favoris et récents sont propres à chaque profil. Les éditions couvertes par les adaptateurs, les permissions et les limites de validation sont détaillées dans [les intégrations](docs/integrations.md). Le nom du fournisseur affiché à partir d’une URL remote n’est pas une connexion à son compte. Le fetch utilise **Git installé sur la machine**, les gestionnaires d’identifiants et l’agent SSH existants, sans invite interactive. **Nouvel onglet → Cloner** permet de sélectionner un dépôt connecté ou une URL HTTPS/SSH, de choisir un nouveau dossier et de l’ouvrir après clonage. Les dépôts accessibles sont consultés dans cet écran dédié ; les paramètres servent à gérer les connexions. Aucune fonctionnalité d’IA n’est fournie.

## Lancer l’application desktop

Préparer Rust avec rustup et les [prérequis GPUI Kit de votre OS](https://gpui-kit.com/docs/installation/), dont Xcode complet sur macOS (les shaders Metal sont compilés à l’exécution par cette version de GPUI Kit). Le toolchain Rust du dépôt est fixé dans `rust-toolchain.toml`.

```sh
cargo run --release --locked --manifest-path src-gpui/Cargo.toml
# Ouverture directe d’un dépôt :
cargo run --release --locked --manifest-path src-gpui/Cargo.toml -- /chemin/du/depot
```

Pour compiler le binaire release sans le lancer :

```sh
cargo build --release --locked --manifest-path src-gpui/Cargo.toml
```

Pour produire une application macOS autonome avec l’icône originale :

```sh
sh src-gpui/scripts/package-macos.sh
open src-gpui/target/release/bundle/Mangrove.app
```

La signature et la notarisation pour une distribution publique restent à configurer. La QA interactive porte sur macOS ARM ; la CI vérifie la compilation et les tests sur les trois OS. Les parcours visuels et le packaging Windows/Linux restent à valider sur ces plateformes. Le [README GPUI](src-gpui/README.md) précise les préférences, les limites et les commandes.

## Vérifier

```sh
cargo fmt --manifest-path src-gpui/Cargo.toml --check
cargo clippy --locked --manifest-path src-gpui/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-gpui/Cargo.toml
cargo fmt --all --check
cargo clippy --locked -p mangrove-core --all-targets -- -D warnings
cargo test --locked -p mangrove-core
cargo clippy --locked -p mangrove-remote --all-targets -- -D warnings
cargo test --locked -p mangrove-remote
```

Les tests du moteur comparent notamment les octets et dates de modification de l’ensemble du dépôt source avant et après consultation. Les tests GPUI protègent les générations de lecture, les ouvertures remplacées, la sélection, les caches bornés et la géométrie du défilement.

Le générateur de fixture nécessite seulement Node.js et Git, sans installation de dépendances JavaScript :

```sh
node --test scripts/create-fixture.test.mjs
node scripts/create-fixture.mjs
```

Le dépôt synthétique est isolé dans `.mangrove-dev/`, ignoré par Git, et n’est pas intégré à l’application. Voir [les mesures de performance](docs/performance.md) pour le générateur de grand historique et le benchmark reproductible.

Pour explorer une longue liste de fichiers et différents types de diffs :

```sh
node scripts/create-fixture.mjs --commits 10000 --large-change
```

L’option ajoute un commit de préparation, puis `feat(fixture): showcase 46 mixed file changes` : 18 fichiers modifiés, 12 ajoutés, 8 supprimés, 6 renommés et 2 changements de type. Le commit contient aussi des fichiers binaires, un fichier vide, un changement de permission, des chemins avec espaces ou accents et un diff de 2 400 lignes pour vérifier la troncature. Il s’ajoute une seule fois à une fixture existante propre sur `main`, sans remplacer son historique.

## Organisation

- `crates/mangrove-core` : lecture Git, graphe, indexation et cache ; indépendant de l’interface.
- `crates/mangrove-remote` : profils, coffre système, lectures HTTP et SSH ; indépendant du moteur Git local.
- `src-gpui` : application native GPUI, vues virtualisées, orchestration des lectures, préférences et packaging ; workspace Cargo indépendant.
- `src-gpui/assets/icons` : icônes originales utilisées pour le packaging.
- `scripts/create-fixture.mjs` : génération isolée de dépôts synthétiques pour la QA et les mesures. Aucun bridge HTTP de lecture des dépôts n’est fourni.

Voir [les décisions d’architecture](docs/architecture.md).

La fidélité, les limites et la comparaison historique des builds release GPUI et Vue/Tauri sont documentées dans le [rapport de performance](docs/performance-gpui.md) ([issue #9](https://github.com/Dpblandin/mangrove/issues/9)). Les sources de l’ancienne interface ont été retirées ; le rapport et les captures restent des preuves de cette comparaison datée.

Les six revues initiales et leurs correctifs sont résumés dans le [rapport historique](docs/review.md). Le thème Macchiato utilise la [palette officielle Catppuccin](https://catppuccin.com/palette/).
