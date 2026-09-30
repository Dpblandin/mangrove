# Mangrove

Application **desktop** pour explorer Git en lecture seule. Tauri embarque l’interface Vue et le moteur Rust dans une application native : la version distribuée ne nécessite ni serveur local, ni navigateur, ni Node.js.

Le cahier des charges complet est dans [l’issue #1](https://github.com/Dpblandin/mangrove/issues/1).

## Prototype disponible

- Ouverture de dépôts locaux, bare et worktrees ; dépôts récemment ouverts.
- Historique topologique paginé, liste virtualisée et graphe Canvas.
- Branches locales, références distantes et tags ; navigation vers leur commit.
- Recherche locale dans les messages, auteurs et emails, avec préfixes de mots ; recherche par préfixe de SHA ou de référence.
- Détails des commits, fichiers modifiés et diffs unifiés à la demande. Les merges sont comparés au premier parent.
- Fichiers ajoutés en vert, supprimés en rouge, modifiés en jaune et renommés en bleu, dans les trois thèmes.
- Un clic sur un fichier ouvre son diff dans la zone centrale, avec les détails et fichiers du commit à droite. La vue compacte reste disponible ; fermer le diff ramène à l’historique à la même position.
- Le message du commit dispose d’une zone de lecture de hauteur stable et défilable : les titres longs, descriptions et notes de fusion ne déplacent pas les métadonnées ni la liste des fichiers.
- Aucun loader animé pendant les lectures de commits ou de diffs : le contenu précédent reste affiché jusqu’à ce que la nouvelle sélection soit prête.
- Indexation progressive dans un cache SQLite FTS5 séparé du dépôt ; réutilisation à la prochaine ouverture.
- Trois thèmes mémorisés : **Classique** par défaut, gris/bleu ; **Mangrove**, vert ; **Catppuccin Macchiato**.
- Fond sombre pour l’historique et les diffs ; Macchiato utilise le même fond que le panneau des détails.
- Panneaux branches et détails redimensionnables à la souris ou au clavier ; leurs largeurs sont mémorisées et les colonnes de l’historique s’adaptent à l’espace disponible. Double clic sur un séparateur pour réinitialiser.

Les intégrations authentifiées GitHub/GitLab/Bitbucket, les profils et le SSH restent à implémenter. Le nom du fournisseur affiché à partir d’une URL remote n’est pas une connexion à son compte. Ce prototype n’effectue ni clone ni fetch et ne contient aucune fonctionnalité d’IA.

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
- `src-tauri` : application desktop, commandes natives de lecture et sélection de dossier.
- `src` : Vue 3, TypeScript, Tailwind CSS 4, historique virtualisé et rendu Canvas.
- `scripts/create-fixture.mjs` : génération isolée de dépôts synthétiques pour la QA et les mesures. Aucun bridge HTTP de lecture des dépôts n’est fourni.

Voir [les décisions d’architecture](docs/architecture.md).

Les six revues et leurs correctifs sont résumés dans [docs/review.md](docs/review.md). Le thème Macchiato utilise la [palette officielle Catppuccin](https://catppuccin.com/palette/).
