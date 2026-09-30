# Mesures initiales

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
vp run fixture --commits 10000
cargo run --release --locked -p mangrove-core --example benchmark -- .mangrove-dev/fixture-10000
```

Le benchmark place son cache dans un dossier temporaire de l’OS et le supprime à la fin. Il peut aussi lire un dépôt local existant en passant son chemin ; il ne modifie pas ce dépôt.

## Prochaines mesures

Avant de fixer des objectifs chiffrés, mesurer plusieurs tailles et vrais dépôts : graphes denses, refs nombreuses, messages longs, fichiers volumineux et changements de références. Ajouter le temps jusqu’au premier affichage natif, la fluidité du scroll et les mémoires RSS/processus WebView. Répéter les ouvertures à cache filesystem froid et chaud. Conserver les percentiles et le contexte matériel pour comparer les futures optimisations.

## Contenu, historique de fichier et blame (issue #3)

Mesure du 30 septembre 2026, Apple M4 Pro, 48 Gio de RAM, macOS ARM, Rust release avec ThinLTO. Le benchmark `file_benchmark` attend la fin de l’indexation avant de mesurer. Chaque opération utilise un processus distinct et un cache applicatif temporaire hors du dépôt source : une première lecture, puis 30 lectures, cache filesystem chaud. Les chiffres concernent le moteur natif ; ils excluent IPC, rendu Vue et coloration Shiki. La QA du bundle natif est décrite dans [qa-file-browser.md](qa-file-browser.md).

- **Fixture synthétique de 10 007 commits**, fichier TypeScript de 6 lignes : contenu p50 **0,23 ms**, p95 **0,29 ms** ; première page d’historique p50 **0,67 ms**, p95 **0,81 ms** ; blame p50 **20,17 ms**, p95 **21,54 ms**. Le premier historique renvoie 40 changements et un curseur ; le blame indique la frontière des 256 commits. Les premières lectures prennent respectivement 0,50, 1,12 et 19,41 ms. Le pic RSS du processus reste entre **27,7 et 27,8 Mio**, baseline après indexation incluse.
- **Dépôt réel local de 12 075 commits indexés**, fichier de 32 lignes : contenu p50 **0,76 ms**, p95 **0,87 ms** ; première page d’historique p50 **5,05 ms**, p95 **5,74 ms** ; blame p50 **107,56 ms**, p95 **115,92 ms**. Les premières lectures prennent respectivement 1,07, 6,18 et 106,83 ms. Les pics RSS sont **45,7 Mio** pour le contenu, **45,4 Mio** pour l’historique et **58,9 Mio** pour le blame, contre **45,4 Mio** pour la baseline après indexation. Le blame atteint une frontière de préparation et fournit une attribution limitée explicitement, plutôt que charger des candidats historiques volumineux.

Ces pics RSS incluent les caches et structures de libgit2 et l’indexation initiale ; ce ne sont pas des tailles de réponse. Le budget de 32 Mio contrôle les blobs distincts préparés, sans garantir une borne globale de RSS. Les échantillons mémoire proviennent d’un seul processus par opération et comportent du bruit entre processus. Les bornes de deux secondes sont vérifiées entre commits, sans interrompre une opération libgit2 déjà engagée. Aucun nom de fichier, chemin local, SHA, auteur ou contenu du dépôt réel n’est publié. Les résultats agrégés sont dans [performance-files.json](performance-files.json).

Protocole reproductible (macOS ; sur Linux, utiliser `/usr/bin/time -v` pour le pic RSS) :

```sh
vp run fixture --commits 10000 --large-change
cargo build --release --locked -p mangrove-core --example file_benchmark
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts content
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts history
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts blame
/usr/bin/time -l target/release/examples/file_benchmark .mangrove-dev/fixture-10000 HEAD src/history.ts baseline
```

Pour un dépôt réel, remplacer seulement le dépôt et le chemin de fichier dans ces commandes. La sortie JSON du programme ne contient que des données agrégées. Conserver les logs locaux hors du dépôt si l’outil externe de mesure y imprime le chemin de la commande.
