# Performance des diffs et de l’historique — issue #17

Corrections et mesures du 1er octobre 2026, comparées à `28825b7001775ac17bfaabbb23c3818537650f3b`. Le périmètre reprend [l’issue](https://github.com/Dpblandin/mangrove/issues/17) et [son complément](https://github.com/Dpblandin/mangrove/issues/17#issuecomment-5935287552).

## Comportement livré

L’inspecteur publie les détails du commit après Git, puis le patch brut, puis les tokens. La coloration possède sa propre tâche ; elle ne retient plus le gate des lectures Git. Les demandes périmées sont rejetées avant la lecture, après celle-ci et lors de la publication. Les analyses vérifient leur génération entre étapes, côtés et hunks. Le clavier attend 70 ms pendant une rafale ; la première touche et les clics restent immédiats.

Tree-sitter réutilise un highlighter par grammaire pendant une analyse. Les arbres et les injections sont remplacés entre fragments. JSONC, JSON5, SCSS et LESS choisissent le tokenizer lexical avant la construction des requêtes. Une annulation ou un délai dépassé ne mémorise plus un résultat vide.

Les détails de commit et les identités de fichiers sont partagés et conservés dans un cache limité à huit commits et 8 Mio estimés. Les identités du préfixe affichable restent utilisables au-delà de 5 000 fichiers. Les patches locaux réutilisent le dernier statut et relisent les chemins sélectionnés ; une identité renommée devenue incompatible retrouve la comparaison complète.

La similarité contrôle les candidats et les headers des objets avant lecture de contenu : 200 candidats, 2 Mio par fichier et 16 Mio cumulés. Lorsque ce budget ne suffit pas, les fichiers restent consultables et une notice indique la limite. Les renommages exacts par OID restent détectables pour les objets Git ; les candidats locaux volumineux ne sont pas hashés pour les comparer. Les patches sélectionnés vérifient aussi la taille avant matérialisation, même avec un attribut `diff=true`.

Les lignes de code affichent au maximum 16 Kio après développement des tabulations, avec un suffixe et une notice. Les octets originaux restent disponibles pour la sélection et la copie. Le paint ne prépare que les glyphes horizontaux visibles ; la géométrie complète du préfixe reste utilisée pour les coordonnées et les fonds de sélection. Les glyphes réordonnés conservent le rendu complet du préfixe borné.

Le graphe rejette les segments hors largeur avant leur construction et applique un masque au canvas. Les segments verticaux utilisent des rectangles. Le moteur partage les identités des voies, réserve les vecteurs à leur taille utile et représente le type d’arête par un enum dont la sérialisation conserve les anciennes chaînes.

Le cache historique conserve douze pages au maximum, avec des budgets de 64 Mio estimés et deux millions d’arêtes. Une seule page utile peut dépasser ces budgets pour préserver la navigation d’un graphe exceptionnel ; les autres sont alors évincées. La destruction des pages est déportée hors du thread UI. Une page distante devenue inutile n’est plus relue après son éviction.

Les résultats de recherche sont matérialisés par requête, parcours publié et nombre de commits indexés. Les pages suivantes ne répètent plus le CTE, le comptage et le tri. Le cache possède quatre entrées et un budget de 16 Mio ; un résultat dépassant 2 097 152 positions est calculé sans être conservé. Une réindexation construit un parcours distinct puis publie les tables et leur version atomiquement. L’ancien historique et les pages filtrées restent visibles jusqu’à remplacement. Les positions de navigation et leur progression proviennent du même snapshot SQLite ; une réponse ancienne ne peut pas restaurer un parcours retiré.

Les sujets, dates, badges et libellés Solo sont préparés à l’acceptation des données. L’inspecteur et le viewer central utilisent le cache de vue existant, qui désactive ce raccourci lorsque l’accessibilité est active. La sidebar ne s’invalide plus entre deux commits sans changement de référence surlignée.

## Mesures de composants

Machine : Apple M4 Pro, 48 Gio, macOS 26.6.2, Rust 1.95.0. Les sondes utilisent des fixtures synthétiques et des caches temporaires. Le cache filesystem et la charge système de fond ne sont pas contrôlés. Les petits échantillons et leurs quantiles sont descriptifs. Aucun de ces timings de composants n’est une mesure de FPS ou de latence écran.

- **150 hunks TypeScript** : vingt processus release après warmup par révision. Coloration p50 **520,95 → 45,95 ms**, p95 **527,00 → 65,63 ms**. L’ancien code abandonne tous les tokens ; le nouveau en conserve 3 900 pour 1 350 lignes de tokens. Lecture Git, queueing et UI exclus. [Échantillons](performance-17/syntax.json).
- **Recherche sur 100 005 commits** : dix lectures chaudes par page. `example`, offset 99 877 : **188,05 → 0,161 ms p50** ; `Alice`, offset 25 001 : **102,01 → 0,192 ms**. Première création du snapshot `example` : **95,27 → 93,84 ms**, donc le coût initial reste présent. [Avant](performance-17/search-before.json), [après](performance-17/search-after.json).
- **Graphe de 128 lignes et 5 000 voies actives** : cinq passes séquentielles optimisées. Allocations appartenant aux pages : **52,27 → 19,54 Mio** ; génération p50 **45,09 → 22,40 ms** ; destruction **4,494 → 0,006 ms**. Les 640 128 arêtes produisent le même digest géométrique. Sur vingt lignes, le culling admet 140 des 100 020 segments aux trois offsets contrôlés. Ces octets sont les capacités d’allocations, pas le RSS. [Résultats](performance-17/graph.json).
- **Expansion de 512 Kio de tabulations** : vingt mesures optimisées après warmup. Préparation p50 **2,889 → 0,0167 ms** ; texte affiché **4 Mio → 16 383 octets**. `SharedString` est remplacé par `Arc<str>` dans cette sonde isolée ; shaping, paint, copie originale et GPUI sont exclus. [Résultats](performance-17/line-expansion.json), [source](performance-17/line-expansion-probe.rs).

## Application GPUI release

Deux séries de six lancements complets : trois caches applicatifs neufs et trois réutilisés par révision, même fixture de 10 005 commits, viewport de 1440 × 888, six interactions alternées, puis soixante sélections à au moins 33 ms, trois secondes de scroll par vue et deux secondes au repos. La baseline reçoit uniquement le [patch d’instrumentation](performance-17/baseline-instrumentation.patch), qui conserve son comportement de chargement. Le profiler de frames et le scénario sont communs ; les compteurs d’opérations de la baseline ne couvrent que les rendus de l’inspecteur.

Les chiffres suivants sont des quantiles descriptifs, avec seulement trois lancements indépendants par condition. Les timings de disponibilité sont observés à la cadence des callbacks, puis à une soumission `Present` ; ils ne mesurent pas clic → photons. Les frames regroupées ne constituent pas autant de répétitions indépendantes. [Avant](performance-17/ui-before.json), [après final](performance-17/ui-after.json), [provenance des sources et du binaire](performance-17/ui-provenance.json).

- **Détails du commit** : p50 groupé **16,69 → 16,67 ms** à cache neuf et **24,69 → 16,74 ms** à cache réutilisé. Les petits timings sont fortement quantifiés par la cadence de l’écran.
- **Dernière sélection de la rafale** : p50 **92,16 → 86,44 ms** et **110,00 → 86,98 ms**. CPU pendant la phase, médiane des moyennes par lancement, en pourcentage d’un cœur : **108,19 → 17,72 %** et **106,38 → 17,97 %**. Le build final lance quatre lectures sur cinq lancements, cinq sur le sixième ; ses colorations périmées sont rejetées. Les compteurs ne permettent pas de comparer le nombre de lectures avec la baseline.
- **Patch brut du grand diff** : disponibilité CPU p50 **83,30 → 25,00 ms** et **83,34 → 25,02 ms**. Le chargement complet avec coloration reste **91,64 → 99,90 ms** à cache neuf et **91,64 → 91,71 ms** à cache réutilisé ; aucun gain de coloration complète n’est établi sur ce fichier.
- **Scroll de l’historique** : `Draw` p95 groupé **4,35 → 3,71 ms** et **4,38 → 3,71 ms**. L’inspecteur passait par `render` à chaque frame (360–361 appels par lancement) ; il ne s’y rend plus pendant cette phase. Son cache de vue est désactivé lorsque l’accessibilité est active.
- **Scroll du diff** : `Draw` p95 **3,45 → 3,18 ms** et **3,46 → 3,46 ms**. Les pics RSS médians de la campagne complète sont **215,47 → 217,81 Mio** et **217,38 → 217,97 Mio** : aucun gain de RSS global n’est établi.

**Coût restant à traiter :** pendant la rafale, `Draw` p95 monte de **2,33 → 5,89 ms** et **2,21 → 5,84 ms** ; pendant la phase de coloration, **1,81 → 2,30 ms** et **1,77 → 2,56 ms**. Le debounce conserve plus longtemps les détails désactivés d’un commit de 34 fichiers, tandis que la baseline publiait les petits commits intermédiaires. Une [première série après modification](performance-17/ui-after-initial.json) atteignait 7,06–7,11 ms en rafale : la correction des notifications répétées ramène les rendus de l’inspecteur de 63 à 4–5 par lancement. Le panneau conservé reste à peindre, mais l’attribution précise du coût résiduel n’a pas été profilée. La liste et ses callbacks gardent des handles cohérents jusqu’aux nouveaux détails. Ces résultats justifient les gains de travail CPU et de disponibilité du patch brut ; ils ne prouvent pas une amélioration générale des frames.

## Reproduction

```sh
node scripts/create-fixture.mjs --commits 100000 --large-change
MANGROVE_PERF_REPOSITORY="$PWD/.mangrove-dev/fixture-100000" \
MANGROVE_PERF_REPORT=/tmp/mangrove-search.json \
  cargo test --release --locked -p mangrove-core --test performance_index \
  benchmark_filtered_pages -- --ignored --nocapture
cargo test --release --locked -p mangrove-core --test performance_diff \
  diff_performance_probe -- --ignored --nocapture
cargo run --release --locked -p mangrove-core --example graph_benchmark -- 5000 128 5
rustc --edition=2024 -O docs/performance-17/line-expansion-probe.rs \
  -o /tmp/mangrove-line-expansion
/tmp/mangrove-line-expansion
```

`node scripts/create-performance-fixture.mjs` crée un nouveau dépôt isolé avec 150 hunks TypeScript, une ligne de 400 Kio, des tabulations, 1 000 renommages, un gros blob, un merge de 40 parents avec au moins 1 000 renommages, 50 000 tags et un long sujet. La commande refuse de réutiliser un dossier existant et renvoie les OID des scénarios. La sonde `inspect-latency` accepte le dépôt, le commit de syntaxe et `many-hunks.ts` ; elle affiche également le coût et l’identité du cache chaud.

Le benchmark GPUI ajoute une rafale de soixante sélections espacées d’au moins 33 ms. Il collecte les frames pendant les sélections, la coloration et le scroll, le délai depuis la dernière action réelle, les lectures lancées/rejetées, les rendus de l’inspecteur par phase et les temps de gate, Git, coloration et préparation UI. Il reste disponible uniquement avec `app-benchmark`. La procédure de lancement reste celle du [protocole GPUI](performance-gpui-protocol.md).

```sh
sh src-gpui/scripts/package-macos.sh --benchmark
python3 -B scripts/benchmark-ui.py \
  --gpui 'src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app/Contents/MacOS/mangrove-gpui' \
  --repository .mangrove-dev/fixture-10000 --samples 3 --interaction-samples 6 \
  --output /tmp/mangrove-ui-current.json
```

## Vérification et limites

Les tests couvrent la coloration multi-hunks et les injections, les générations de sélection et de coloration, les chemins littéraux et non UTF-8, les renommages et les attributs Git, l’absence de lectures de blobs sans rapport, la migration SQLite et ses aliases, la publication atomique de parcours, les positions périmées, les budgets de caches, la copie du contenu masqué et la géométrie de sélection/défilement.

Contrôles locaux : `cargo fmt` et Clippy avec `-D warnings` sur tous les targets du moteur, de GPUI production et de GPUI `app-benchmark` ; 88 tests moteur réussis et deux sondes ignorées exécutées séparément ; 120 tests GPUI production et 126 avec instrumentation, plus seize tests de l’exemple dans chaque configuration. Les tests des générateurs de fixture et du runner passent ; trois contrôles spécifiques Linux sont ignorés sur macOS. Les revues déléguées `refactor` puis `simplify` couvrent les six périmètres, avec intégration et vérification des corrections.

QA native sur macOS, avec accessibilité active et préférences isolées : ouverture du dépôt à 10 005 commits, sélection d’un fichier, bascules Diff → Fichier → Historique → Blame → Diff et retour de la coloration ; ouverture de la fixture de stress, graphe du merge à quarante parents et décalage horizontal sans débordement sur les sujets ; notice de similarité limitée et accès au patch d’un fichier parmi les 2 002 ajouts/suppressions ; notice de la ligne minifiée, recherche `syntax`, entrée et sortie de Solo. Pendant la reconstruction Solo, les anciennes lignes restent affichées jusqu’à publication des deux commits du parcours, puis le parcours complet revient à la sortie.

Après le dernier ajustement des notifications, le binaire final SHA-256 `6807713aff5f36c2594de4ad2efefadf4b7aae9e9e768c887f26a9b2b5399b27` a aussi été vérifié avec accessibilité active : plusieurs sélections successives publient le SHA et les fichiers de la dernière demande, puis `many-hunks.ts` affiche les numéros ancien/nouveau et la coloration du diff à 150 hunks. Le dépôt de stress reste sans changements locaux après ces lectures.

La QA manuelle des raccourcis, du glisser et du presse-papiers reste incomplète : l’outil de contrôle natif échoue à activer le bundle de QA pour les événements clavier (`procNotFound`, erreur LaunchServices), malgré plusieurs modes de lancement. Les actions d’accessibilité fonctionnent. La sélection/copie complète est couverte par les tests Rust ; la rafale instrumentée utilise le gestionnaire de sélection clavier et valide l’identité finale, mais ne remplace pas une vérification des touches physiques.

Les protections existantes restent en place : listes virtualisées, pages de 128 commits, lectures physiques bornées, partage par `Arc` et géométrie de scroll logarithmique. Une opération libgit2, la compilation d’une requête ou le traitement d’un fragment déjà engagés ne sont pas préemptés au milieu. Les limites de temps ne constituent donc pas des deadlines murales strictes.

Les connexions SQLite inactives sont conservées uniquement sur Unix. Ailleurs, elles sont fermées comme auparavant pour préserver la suppression du cache, notamment avec les modes de partage de fichiers de SQLite sur Windows.

Les tests et mesures présentés ont été exécutés sur macOS. Les builds Windows/Linux et une campagne complète de frames sur tous les scénarios extrêmes restent à confirmer. Les gains de composants ne permettent pas de déduire ceux des frames de ces scénarios.
