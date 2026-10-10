# Mesures du moteur Git

Ce document conserve les mesures datées du moteur `mangrove-core`, indépendant de l'interface. Les commandes ci-dessous utilisent Cargo ; le générateur de fixture nécessite Node.js et Git, sans installation npm. La comparaison historique des interfaces est conservée séparément dans [performance-gpui.md](performance-gpui.md), avec ses limites de protocole.

Les corrections de l’audit des diffs et de l’historique, leurs mesures avant/après et la vérification de l’application courante sont décrites dans [performance-17.md](performance-17.md).

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

Avant de fixer des objectifs chiffrés, mesurer plusieurs tailles et vrais dépôts : graphes denses, refs nombreuses, messages longs et changements de références. Les diffs de fichiers volumineux sont mesurés [plus bas](#diffs-de-fichiers-volumineux). Pour les campagnes GPUI, mesurer le temps jusqu’au premier affichage natif, la fluidité du scroll et les mémoires RSS du processus. Répéter les ouvertures à cache filesystem froid et chaud. Conserver les percentiles et le contexte matériel pour comparer les futures optimisations. Les benchmarks du moteur seul n'incluent pas ces coûts de rendu.

## Diffs de fichiers volumineux

Mesure du 10 octobre 2026, Apple M4 Pro, 48 Gio de RAM, macOS ARM, Rust release avec ThinLTO. Pour chaque cas, `large_diff_benchmark` crée un dépôt de deux commits, puis lit dix fois le diff du fichier avec `Workspace::file_diff` dans un processus distinct, après indexation. Le temps est la médiane des dix lectures ; la mémoire est le pic RSS du processus moins son RSS avant la première lecture. Les objets sont loose et le cache de l’OS est chaud. Ces chiffres concernent le moteur seul : l’interface reçoit au plus 2 000 lignes et 512 Kio de patch, déjà mesurés dans [performance-gpui.md](performance-gpui.md).

La borne historique de 2 Mio n’avait pas été mesurée avant d’être posée. Le coût d’un diff croît linéairement, de 4 à 12 ms par Mio selon le fichier et le changement ; à 2 Mio, il reste sous 20 ms. Le fichier `docs/performance-gpui.html` de 5,4 Mio, dont seize lignes changent à `ad0604e`, se lit en 38 ms avec 31 à 39 Mio de mémoire supplémentaire. La limite d’aperçu vaut donc 8 Mio par défaut : au plus 75 ms et 64 Mio dans les cas mesurés. Elle se règle jusqu’à 64 Mio.

| Fichier et changement                   | Taille | Temps p50 | Mémoire, `git2::Patch` | Mémoire, en flux |
| --------------------------------------- | -----: | --------: | ---------------------: | ---------------: |
| Texte, 32 lignes modifiées              |  8 Mio |     63 ms |                 53 Mio |           58 Mio |
| Texte entièrement réécrit               |  8 Mio |     69 ms |                131 Mio |           64 Mio |
| Texte, 32 lignes modifiées              | 64 Mio |    584 ms |                352 Mio |          349 Mio |
| Texte ajouté                            | 64 Mio |    303 ms |                538 Mio |          216 Mio |
| Texte entièrement réécrit               | 64 Mio |    657 ms |                994 Mio |          354 Mio |
| Lignes répétitives, une sur dix changée | 64 Mio |    792 ms |                509 Mio |          280 Mio |
| Lignes de 64 Kio, 32 modifiées          | 64 Mio |    531 ms |                180 Mio |          180 Mio |

Les temps sont ceux de la version en flux ; ils restent à 8 % près ceux de `git2::Patch`. Un `git2::Patch` conserve un enregistrement par ligne du patch, alors que l’interface n’en affiche que 2 000. Sur macOS, ces tableaux qui grandissent par réallocations fragmentent la mémoire : chaque lecture d’une réécriture de 64 Mio laissait environ 64 Mio de plus dans le RSS, d’où les 994 Mio après dix lectures. Les diffs entre deux blobs (commits, index, stashes et révisions de l’historique) passent maintenant par `Repository::diff_blobs` : les lignes sont comptées au passage et seules les lignes affichées sont gardées. Le RSS reste alors stable d’une lecture à l’autre. Le reste du coût vient des deux blobs lus et des structures de xdiff, proportionnelles à la taille. Les modifications non indexées comparent un blob à un fichier local, que git2 ne sait pas lire en flux : elles conservent `git2::Patch`.

Les 93 cas mesurés avec `git2::Patch` et les 84 cas en flux sont dans [performance-large-files.json](performance-large-files.json) : textes, lignes de 64 Kio, fichiers d’une ligne et lignes répétitives, modifiés, ajoutés, réécrits ou changés une ligne sur dix, de 1 à 64 Mio. Le premier passage a été mesuré avec la garde de 2 Mio relevée à 1 Gio, jusqu’à 128 Mio.

```sh
cargo build --release --locked -p mangrove-core --example large_diff_benchmark
target/release/examples/large_diff_benchmark fixture /tmp/large-rewrite 64 text rewrite
/usr/bin/time -l target/release/examples/large_diff_benchmark case /tmp/large-rewrite
```

## Avance et retard des branches suivies

Mesure du 10 octobre 2026, Apple M4 Pro, 48 Gio de RAM, macOS ARM, Rust release avec ThinLTO. `scripts/create-tracking-fixture.mjs` crée un dépôt de 121 227 commits, dont 104 799 accessibles depuis main, et 5 000 branches locales suivies. Le générateur place 80 % des branches près de leur upstream (jusqu’à 10 commits en avance et 40 commits de main en retard), 15 % de 1 000 à 10 000 commits en retard, 2,5 % à environ 50 000 et 2,5 % sur un historique sans ancêtre commun. Des merges octopus élargissent les 20 000 derniers commits et gonflent les retards : 3 715 paires (74 %) comptent 50 commits au plus, 876 de 51 à 10 000 et 409 davantage. Le pack est réécrit comme par `git gc`. `tracking_benchmark` attend la fin de l’indexation, puis mesure dans un même processus, avec le cache de l’OS chaud : 30 résolutions des upstreams, chaque paire seule avec `graph_ahead_behind` sur un même dépôt libgit2, puis toutes les paires par appels de 100 ms comme l’interface, à froid, à chaud et après le déplacement d’un upstream sur cinquante. Ces chiffres concernent le moteur seul.

| Opération                                        |             Sans commit-graph |             Avec commit-graph |
| ------------------------------------------------ | ----------------------------: | ----------------------------: |
| Résolution des 5 000 upstreams, p50 / p95        |                40,4 / 41,6 ms |                41,2 / 50,3 ms |
| Paire de 50 commits au plus, p50 / p95           |              0,014 / 0,038 ms |              0,014 / 0,038 ms |
| Paire de 51 à 10 000 commits, p50 / p95          |                  1,1 / 3,5 ms |                  1,1 / 3,5 ms |
| Paire de plus de 10 000 commits, p50 / p95 / max |              20 / 45 / 169 ms |              20 / 45 / 181 ms |
| Comptage à froid par appels de 100 ms            | 10,2 s, 87 appels, max 232 ms | 10,4 s, 91 appels, max 226 ms |
| Comptage à chaud                                 |              0,64 ms, 1 appel |              0,65 ms, 1 appel |
| Après le déplacement de 100 upstreams            |              0,57 s, 4 appels |              0,58 s, 4 appels |

Sur cette fixture, aucune paire n’atteint une seconde : un parcours interruptible propre à Mangrove ou un commit-graph écrit par l’application ne sont pas nécessaires à cette échelle. La paire la plus lente, entre 169 et 212 ms selon les exécutions, est la première paire longue du processus, avant que libgit2 garde les commits lus dans son cache d’objets. Les 409 paires de plus de 10 000 commits représentent 88 % du comptage à froid. Un appel dépasse sa deadline de la durée d’une paire, hors du thread d’interface.

Sur une copie agrandie à 471 228 commits, une paire distante d’environ 350 000 commits prend environ 1,37 s. Elle est comptée en arrière-plan, sans bloquer l’interface ni les autres badges, mais dépasse la seconde : un parcours interruptible ou un commit-graph écrit par l’application restent des suites possibles. Sur cette copie, l’indexation complète prend de 180 à 200 s, avec ou sans comptage en parallèle.

Le commit-graph ne change pas ces temps. libgit2 y lit les parents sans décompresser les commits, mais son comptage visite de toute façon chaque commit entre les deux pointes et leur base commune, et le cache d’objets rend la lecture des commits suivants peu coûteuse. Le fetch de Mangrove, qui n’écrit pas de commit-graph, ne perd donc rien ici. Rouvrir le dépôt à chaque appel perdait ce cache : une première version comptait la même fixture en 76 s, avant réécriture du pack ; le dépôt reste désormais ouvert jusqu’à la fin d’un comptage.

Le pic RSS du processus atteint 254 Mio, indexation de 121 000 commits et deux dépôts libgit2 compris ; le cache d’objets de libgit2 est borné à 256 Mio par défaut pour tout le processus. Les machines plus lentes, le cache de l’OS froid et les dépôts réels restent à mesurer. Les sorties agrégées sont dans [performance-tracking.json](performance-tracking.json).

```sh
node scripts/create-tracking-fixture.mjs .mangrove-dev/tracking
node scripts/create-tracking-fixture.mjs .mangrove-dev/tracking-graph --commit-graph
cargo build --release --locked -p mangrove-core --example tracking_benchmark
/usr/bin/time -l target/release/examples/tracking_benchmark .mangrove-dev/tracking
/usr/bin/time -l target/release/examples/tracking_benchmark .mangrove-dev/tracking-graph
```

Le générateur refuse un dossier existant. Le benchmark lit aussi un dépôt local existant sans le modifier ; il ne publie ni chemin, ni nom de branche, ni SHA.

## Contenu, historique de fichier et blame

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

## Worktrees, stashes et Solo

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
