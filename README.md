# Mangrove

Application **desktop** pour explorer Git. La consultation conserve les fichiers et l’index ; l’auto-fetch met à jour les références distantes. Tauri embarque l’interface Vue et le moteur Rust dans une application native : la version distribuée ne nécessite ni serveur local, ni navigateur, ni Node.js.

Le cahier des charges complet est dans [l’issue #1](https://github.com/Dpblandin/mangrove/issues/1).

## Prototype disponible

- Ouverture de dépôts locaux, bare et worktrees ; dépôts récemment ouverts.
- Onglets de dépôts avec sélection, fermeture et bouton d’ouverture. Chaque changement recharge le dépôt actif ; les onglets conservent uniquement son nom et son chemin.
- Accueil simple : logo Mangrove, ouverture d’un dépôt et accès aux dépôts récents.
- Historique topologique paginé, liste virtualisée et graphe Canvas.
- Branches locales, références distantes et tags ; dossiers imbriqués repliables et navigation vers leur commit. Le filtre révèle les dossiers des références correspondantes sans perdre les replis de la navigation normale.
- Mode **Solo** sur une ou plusieurs branches locales/distantes, accessible par clic droit ou bouton clavier : parcours et graphe recalculés, recherche limitée à ce parcours et retour explicite à la vue complète. La branche réellement active reste indiquée.
- Sections **Worktrees** et **Stashes** chargées à la demande ; ouverture des worktrees existants en onglets, états manquant/verrouillé/détaché, et consultation séparée des diffs sauvegardés du worktree, de l’index et des fichiers non suivis. [Comportements et limites](docs/repository-context.md).
- Les références techniques `refs/t3/checkpoints/` sont masquées dans les branches, l’historique, le graphe et la recherche ; les instantanés restent dans le dépôt pour T3 Code.
- Recherche locale dans les messages, auteurs et emails, avec préfixes de mots ; recherche par préfixe de SHA ou de référence.
- Détails des commits, liste de fichiers virtualisée et diffs unifiés à la demande. Les merges sont comparés au premier parent.
- Consultation du contenu complet à la révision sélectionnée, historique paginé suivant les renommages détectés et blame à la demande ; chemin et SHA conservés entre les vues. Les groupes de blame ouvrent le commit concerné. Rendu virtualisé, coloration et police partagées avec les diffs ; [limites et comportements](docs/file-browser.md) documentés.
- Consultation des modifications non commitées : fichiers indexés, non indexés et non suivis, avec leurs diffs en lecture seule. Le statut est actualisé à l’ouverture du dépôt, au retour du focus dans l’application et par une actualisation manuelle. Aucun bouton ne permet de préparer un fichier pour un commit, de désindexer un fichier ou de créer un commit.
- Fichiers ajoutés en vert, supprimés en rouge, modifiés en jaune et renommés en bleu, dans les trois thèmes.
- Icônes de statut distinctes et chemins compacts dans les listes de fichiers. Les diffs gardent des fonds vert et rouge marqués, avec une coloration syntaxique préchargée progressivement pour le commit ou les modifications non commitées sélectionnés ; le texte ordinaire reste clair. Les fichiers indexés et non indexés sont préparés séparément et chaque actualisation invalide les anciens patches et couleurs.
- Le bouton « + » ouvre les dépôts récents dans un nouvel onglet. Les onglets rechargent le dépôt à l’activation ; le loader apparaît seulement si l’ouverture dépasse 500 ms.
- Un clic sur un fichier ouvre son diff dans la zone centrale, avec les détails et fichiers du commit à droite. La vue compacte reste disponible ; fermer le diff ramène à l’historique à la même position.
- Le message du commit dispose d’une zone de lecture de hauteur stable et défilable : les titres longs, descriptions et notes de fusion ne déplacent pas les métadonnées ni la liste des fichiers.
- Aucun loader animé pendant les lectures de commits ou de diffs : le contenu précédent reste affiché jusqu’à ce que la nouvelle sélection soit prête.
- Indexation progressive dans un cache SQLite FTS5 séparé du dépôt ; réutilisation à la prochaine ouverture.
- Trois thèmes mémorisés : **Classique** par défaut, gris/bleu ; **Mangrove**, vert ; **Catppuccin Macchiato**.
- Paramètres dans une vue dédiée avec navigation par rubrique, accessibles par **Cmd + ,** sur macOS ou **Ctrl + ,** sur Windows/Linux. **Échap** ou le bouton de retour retrouve l’espace de travail précédent.
- Auto-fetch des remotes du dépôt actif : dans **Paramètres → Général**, intervalle mémorisé de **0 à 60 minutes**, **1 minute** par défaut et **0** pour désactiver. L’historique et les références sont actualisés en conservant la recherche et la sélection. Un bouton dans la barre supérieure permet un fetch immédiat, même lorsque l’auto-fetch est désactivé. Les erreurs sont affichées sans interrompre les autres remotes ; les objets LFS ne sont pas récupérés automatiquement.
- Fond sombre pour l’historique et les diffs ; Macchiato utilise le même fond que le panneau des détails.
- Panneaux branches et détails redimensionnables à la souris ou au clavier ; leurs largeurs sont mémorisées et les colonnes de l’historique s’adaptent à l’espace disponible. Double clic sur un séparateur pour réinitialiser.
- Taille des textes mémorisée : **Cmd/Ctrl + +** (ou **=**) agrandit, **Cmd/Ctrl + −** réduit, **Cmd/Ctrl + 0** rétablit la taille par défaut, augmentée d’un pixel par rapport à la première version du prototype.
- Le texte des diffs démarre à **11 px**. Les boutons **− / +** dans leur en-tête règlent uniquement cette taille, mémorisée et partagée entre les vues centrale et compacte. Les raccourcis globaux ajustent les deux tailles ; **Cmd/Ctrl + 0** rétablit aussi le diff à 11 px.

Les intégrations de comptes GitHub/GitLab/Bitbucket et les profils restent à implémenter. Le nom du fournisseur affiché à partir d’une URL remote n’est pas une connexion à son compte. Le fetch utilise **Git installé sur la machine**, les gestionnaires d’identifiants et l’agent SSH existants, sans invite interactive ; aucun clone ni fonctionnalité d’IA n’est fourni.

## Lancer l’application desktop

Préparer [Vite+](https://viteplus.dev/guide/), Rust avec rustup et les [prérequis Tauri de votre OS](https://v2.tauri.app/start/prerequisites/). Le toolchain Rust du dépôt est fixé dans `rust-toolchain.toml`.

```sh
vp install --frozen-lockfile
vp run desktop
```

`vp run dev` lance également la fenêtre desktop. Le serveur Vite démarre uniquement pour recharger l’interface pendant le développement ; les lectures Git passent par les commandes natives Tauri.

Pour produire une application macOS autonome :

```sh
vp run tauri build --bundles app
open target/release/bundle/macos/Mangrove.app
```

`vp run tauri build` produit les formats de distribution de l’OS courant. La signature et la notarisation pour une distribution publique restent à configurer. Le prototype a été compilé et vérifié sur macOS ARM ; les vérifications des autres OS sont configurées en CI.

Toutes les commandes de dépendances passent par `vp` (`vp install`, `vp add`, `vp remove`). Vite+ délègue ici à npm 11.19.0, fixé dans `devEngines.packageManager`, et utilise `package-lock.json` ; `vp` n’a pas son propre format de lockfile. Voir sa [documentation de gestion des paquets](https://viteplus.dev/guide/install).

## Vérifier

```sh
vp run check
vp test run
vp run test:fixture
vp run build
cargo fmt --all --check
cargo clippy --locked -p mangrove-core --all-targets -- -D warnings
cargo test --locked -p mangrove-core
```

Les tests du moteur comparent notamment les octets et dates de modification de l’ensemble du dépôt source avant et après consultation. Les tests Vue vérifient les courses entre ouvertures, recherches, indexation et sélections.

Un dépôt Git synthétique de développement peut être créé avec `vp run fixture`. Il est isolé dans `.mangrove-dev/`, ignoré par Git, et n’est pas intégré à l’application. Voir [les mesures de performance](docs/performance.md) pour le générateur de grand historique et le benchmark reproductible.

Pour explorer une longue liste de fichiers et différents types de diffs :

```sh
vp run fixture --commits 10000 --large-change
```

L’option ajoute un commit de préparation, puis `feat(fixture): showcase 46 mixed file changes` : 18 fichiers modifiés, 12 ajoutés, 8 supprimés, 6 renommés et 2 changements de type. Le commit contient aussi des fichiers binaires, un fichier vide, un changement de permission, des chemins avec espaces ou accents et un diff de 2 400 lignes pour vérifier la troncature. Il s’ajoute une seule fois à une fixture existante propre sur `main`, sans remplacer son historique.

## Organisation

- `crates/mangrove-core` : lecture Git, graphe, indexation et cache ; indépendant de l’interface et du transport.
- `src-tauri` : application desktop, commandes natives de consultation et de fetch, sélection de dossier.
- `src` : Vue 3, TypeScript, Tailwind CSS 4, historique virtualisé et rendu Canvas.
- `scripts/create-fixture.mjs` : génération isolée de dépôts synthétiques pour la QA et les mesures. Aucun bridge HTTP de lecture des dépôts n’est fourni.

Voir [les décisions d’architecture](docs/architecture.md).

Les six revues et leurs correctifs sont résumés dans [docs/review.md](docs/review.md). Le thème Macchiato utilise la [palette officielle Catppuccin](https://catppuccin.com/palette/).
