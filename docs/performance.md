# Mesures du moteur Git

Ce document conserve les mesures datées du moteur `mangrove-core`, indépendant de l'interface. Les commandes ci-dessous utilisent Cargo ; le générateur de fixture nécessite Node.js et Git, sans installation npm. La comparaison historique des interfaces est conservée séparément dans [performance-gpui.md](performance-gpui.md), avec ses limites de protocole.

Les corrections de l’audit #17, leurs mesures avant/après et la vérification de l’application courante sont décrites dans [performance-17.md](performance-17.md).

## Ouverture, indexation et recherche

Mesure du 30 septembre 2026, Apple M4 Pro / macOS ARM, compilation Rust release avec ThinLTO. Dépôt synthétique de 10 005 commits, fichiers textuels petits, branches, tag et merge à trois parents.

Le cache SQLite est neuf pour le premier parcours ; le cache filesystem de l’OS est chaud. Une seule exécution de chargement et 30 échantillons par recherche. Ces mesures excluent le rendu de l’interface, le temps de saisie, les appels natifs et le réseau.

| Opération                                           | Temps observé |
| --------------------------------------------------- | ------------: |
| Retour de l’ouverture, avant indexation             |       3,39 ms |
| Première page de 256 commits, depuis l’ouverture    |      54,30 ms |
| Index complet de 10 005 commits, depuis l’ouverture |     328,75 ms |
| Réouverture avec cache + première page              |       1,72 ms |
| Détails du premier commit + premier diff            |       0,57 ms |

| Recherche      | Résultats |     p50 |     p95 |
| -------------- | --------: | ------: | ------: |
| `fix`          |     1 668 | 2,23 ms | 2,75 ms |
| `feat`         |     3 340 | 3,00 ms | 3,15 ms |
| `cache`        |     1 666 | 2,36 ms | 2,70 ms |
| Préfixe de SHA |         1 | 3,09 ms | 3,53 ms |
| `Alice Martin` |     3 335 | 3,37 ms | 3,53 ms |

La sortie brute est conservée dans [performance-10005.json](performance-10005.json).

Chaque recherche inclut le total de résultats et la lecture des 256 premières lignes. Le SHA dépend du générateur de fixture. Les chiffres ne décrivent pas des fichiers lourds, des dépôts géants ni le pire cas du graphe.

## Reproduire

```sh
node scripts/create-fixture.mjs --commits 10000
cargo run --release --locked -p mangrove-core --example benchmark -- .mangrove-dev/fixture-10000
```

Le benchmark place son cache dans un dossier temporaire de l’OS et le supprime à la fin. Il peut aussi lire un dépôt local existant en passant son chemin ; il ne modifie pas ce dépôt.

## Prochaines mesures

Avant de fixer des objectifs chiffrés, mesurer plusieurs tailles et vrais dépôts : graphes denses, refs nombreuses, messages longs, fichiers volumineux et changements de références. Pour les campagnes GPUI, mesurer le temps jusqu’au premier affichage natif, la fluidité du scroll et les mémoires RSS du processus. Répéter les ouvertures à cache filesystem froid et chaud. Conserver les percentiles et le contexte matériel pour comparer les futures optimisations. Les benchmarks du moteur seul n'incluent pas ces coûts de rendu.

## Contenu, historique de fichier et blame (issue #3)

Mesure du 30 septembre 2026, Apple M4 Pro, 48 Gio de RAM, macOS ARM, Rust release avec ThinLTO. Le benchmark `file_benchmark` attend la fin de l’indexation avant de mesurer. Chaque opération utilise un processus distinct et un cache applicatif temporaire hors du dépôt source : une première lecture, puis 30 lectures, cache filesystem chaud. Les chiffres concernent le moteur natif ; ils excluent le transport vers l'interface, son rendu et sa coloration. La QA historique du bundle de l'époque est décrite dans [qa-file-browser.md](qa-file-browser.md) ; la QA GPUI est décrite dans le [README natif](../src-gpui/README.md).

- **Fixture synthétique de 10 007 commits**, fichier TypeScript de 6 lignes : contenu p50 **0,23 ms**, p95 **0,29 ms** ; première page d’historique p50 **0,67 ms**, p95 **0,81 ms** ; blame p50 **20,17 ms**, p95 **21,54 ms**. Le premier historique renvoie 40 changements et un curseur ; le blame indique la frontière des 256 commits. Les premières lectures prennent respectivement 0,50, 1,12 et 19,41 ms. Le pic RSS du processus reste entre **27,7 et 27,8 Mio**, baseline après indexation incluse.
- **Dépôt réel local de 12 075 commits indexés**, fichier de 32 lignes : contenu p50 **0,76 ms**, p95 **0,87 ms** ; première page d’historique p50 **5,05 ms**, p95 **5,74 ms** ; blame p50 **107,56 ms**, p95 **115,92 ms**. Les premières lectures prennent respectivement 1,07, 6,18 et 106,83 ms. Les pics RSS sont **45,7 Mio** pour le contenu, **45,4 Mio** pour l’historique et **58,9 Mio** pour le blame, contre **45,4 Mio** pour la baseline après indexation. Le blame atteint une frontière de préparation et fournit une attribution limitée explicitement, plutôt que charger des candidats historiques volumineux.

Ces pics RSS incluent les caches et structures de libgit2 et l’indexation initiale ; ce ne sont pas des tailles de réponse. Le budget de 32 Mio contrôle les blobs distincts préparés, sans garantir une borne globale de RSS. Les échantillons mémoire proviennent d’un seul processus par opération et comportent du bruit entre processus. Les bornes de deux secondes sont vérifiées entre commits, sans interrompre une opération libgit2 déjà engagée. Aucun nom de fichier, chemin local, SHA, auteur ou contenu du dépôt réel n’est publié. Les résultats agrégés sont dans [performance-files.json](performance-files.json).

Protocole reproductible (macOS ; sur Linux, utiliser `/usr/bin/time -v` pour le pic RSS) :

```sh
node scripts/create-fixture.mjs --commits 10000 --large-change
cargo build --release --locked -p mangrove-core --example file_benchmark
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts content
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts history
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts blame
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts baseline
```

Pour un dépôt réel, remplacer seulement le dépôt et le chemin de fichier dans ces commandes. La sortie JSON du programme ne contient que des données agrégées. Conserver les logs locaux hors du dépôt si l’outil externe de mesure y imprime le chemin de la commande.

## Worktrees, stashes et Solo (issue #4)

Mesure du 30 septembre 2026, Apple M4 Pro, 48 Gio de RAM, macOS ARM, Rust release avec ThinLTO. Le benchmark `context_benchmark` termine l’indexation initiale, puis mesure 30 lectures de chaque opération dans un même processus, avec cache filesystem chaud et cache applicatif temporaire hors du dépôt. Le changement Solo alterne entre une branche et deux références distinctes, attend la fin du nouveau parcours et lit une page de 128 commits. Les métadonnées SQLite sont réutilisées. Les chiffres excluent le transport vers l'interface, son rendu et sa coloration.

- **Fixture synthétique de 10 007 commits**, quatre worktrees dont un absent et un verrouillé, un stash avec trois zones : worktrees p50 **0,91 ms**, p95 **1,23 ms** ; stashes **0,17 / 0,23 ms** ; détails des trois fichiers du stash **0,61 / 0,72 ms** ; patch de quatre lignes **0,67 / 0,83 ms**. Recalcul Solo et première page : **119,81 / 136,34 ms**. Les deux filtres de cette fixture atteignent le même historique complet. Pic RSS du processus : **27,81 Mio**.
- **Dépôt réel local de 12 090 commits indexés**, 44 worktrees et neuf stashes : worktrees p50 **34,81 ms**, p95 **45,86 ms** ; stashes **0,72 / 0,80 ms** ; détails de deux fichiers **23,80 / 24,55 ms** ; patch de 62 lignes **29,81 / 31,62 ms**. Recalcul Solo et première page : **65,64 / 69,93 ms**, avec 3 594 commits dans le dernier filtre. Pic RSS : **53,42 Mio**.

Dans ces couples, le premier chiffre est le p50 et le second le p95. Les premiers appels sont conservés séparément dans [performance-context.json](performance-context.json), mais font partie des 30 échantillons. Le pic RSS inclut l’indexation initiale et toutes les opérations du processus ; il n’isole ni une réponse ni le coût mémoire du Solo. Les réponses sont bornées, sans borne globale sur les allocations internes de libgit2, notamment le reflog, les arbres et la détection de renommages. Ces deux mesures ne représentent pas un pire cas. Aucun chemin, SHA, nom de branche, auteur ou contenu du dépôt réel n’est publié.

```sh
node scripts/create-fixture.mjs --commits 10000 --large-change
# Préparer des worktrees et un stash dans une copie synthétique de cette fixture.
cargo build --release --locked -p mangrove-core --example context_benchmark
/usr/bin/time -l target/release/examples/context_benchmark /chemin/vers/la/copie
```

La mesure des diffs est omise si aucun stash ou fichier n’est présent ; l’alternance Solo est omise si le dépôt possède moins de deux références de branches distinctes. Sur Linux, remplacer `time -l` par `time -v`. La QA historique du bundle de l'époque est décrite dans [qa-repository-context.md](qa-repository-context.md) ; la QA GPUI est décrite dans le [README natif](../src-gpui/README.md).
