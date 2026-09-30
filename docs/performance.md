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
