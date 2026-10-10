# Résultats de la comparaison Vue/Tauri et GPUI

**Comparaison historique archivée.** Les 120 mesures restent associées à leur provenance d’origine : base `dbce1c713858dd02ed294eaaf85bed676f4e42d5` et hash de l’arbre source alors non committé, conservés dans le JSON. Le code comparatif complet avant retrait de Tauri est disponible à `0476530b84bbd4bb36c8592cded6b85ad65127f6`, avec des correctifs ultérieurs ; cette révision n’est pas celle de l’arbre mesuré. Le runner courant mesure GPUI seul. Les chiffres ci-dessous ne sont pas des mesures du binaire courant.

Le gain principal est la mémoire RSS : la médiane stable passe de **556 à 199 MiB** avec un cache neuf, et de **554 à 187 MiB** avec un cache réutilisé, soit **64 à 66 % de moins**. Les tableaux décrivent séparément la réactivité et le coût des phases : le CPU baisse dans l’historique et augmente dans le grand diff. Ces chiffres comparent les applications complètes et leurs choix de rendu, avec le même moteur Git.

Ces résultats ne permettent pas de conclure que GPUI est plus fluide dans tous les parcours. La fixture inspectée comporte 46 fichiers et son grand diff revient au texte simple. Les commits réels avec beaucoup de fichiers, la navigation entre des diffs non préchargés et le défilement de grands diffs colorés demandent des contrôles distincts.

Les deux artefacts mesurés étaient des builds **release avec `app-benchmark` activé**, utilisant les sources GPUI production `src-gpui` et le baseline Vue/Tauri désormais archivé. Tauri activait aussi le frontend via `VITE_APP_BENCHMARK=1`. Le coût du scénario et du profiler GPUI entre dans ces mesures. Les builds ordinaires de cette révision compilaient les interfaces sans les modules de mesure ; les noms et identifiants des bundles de benchmark étaient distincts.

Les deux familles de lockfiles utilisent les mêmes versions des dépendances du moteur partagé : `git2 0.21.0`, `libgit2-sys 0.18.8+1.9.7`, `rusqlite 0.38.0`, `libsqlite3-sys 0.36.0`, `serde 1.0.229`, `serde_json 1.0.151`, `regex 1.13.1`, `chrono 0.4.45`.

## Série et environnement

Les [données complètes](performance-gpui.json) contiennent **120 lancements réussis**, 30 par application et condition de cache. Chaque lancement réalise 30 sélections de commit, 30 petits diffs effectivement colorés, trois secondes de défilement du grand diff, trois secondes de défilement de l’historique et deux secondes au repos. Cela représente 3600 sélections de détails et 3600 sélections de diff coloré.

- Apple M4 Pro, 48 GiB de RAM, macOS 26.6.2, alimentation secteur. Applications habituelles laissées ouvertes ; charge de fond non neutralisée. Aucun build ni intervention sur les fenêtres mesurées.
- Écran principal interne : 1512 × 982 pixels logiques, 3024 × 1964 physiques, échelle 2, fréquence déclarée de 120 Hz. Deux écrans externes connectés ; chaque lancement est attesté actif et entièrement sur le principal.
- Fenêtre : 1440 × 920 pixels logiques, contenu réel validé de 1440 × 888, thème Classique et polices par défaut.
- Fixture synthétique propre : 10 007 commits, 46 fichiers modifiés dans le commit inspecté, grand patch limité par le moteur à 2000 lignes. Ce grand patch revient au texte simple des deux côtés ; les petits diffs mesurés produisent des tokens colorés.
- Nouvelle instance activée via l’API publique LaunchServices/NSWorkspace, PID exact retourné, ordre des applications alterné. Le workload s’exécute dans les applications ; aucun clic, clavier ou navigateur externe ne le pilote.
- Cache neuf = SQLite propre à chaque application et paire ; cache réutilisé = seconde exécution de la paire. Le cache filesystem reste chaud ou incontrôlé ; les processus recréent leurs caches mémoire et workers.

Les quantiles utilisent le rang le plus proche. Pour les opérations répétées, les p50/p95 ci-dessous décrivent la distribution des **médianes par lancement**, soit 30 unités indépendantes par condition, et non le p95 de toutes les interactions regroupées. Ces dernières restent disponibles dans le JSON. Le [protocole](performance-gpui-protocol.md) précise les critères, les caches et la reproduction.

## Démarrage jusqu’à la vue utile

En ms, **p50 / p95**. La vue utile exige dix lignes visibles chargées, le graphe, les détails du premier commit et le bon viewport. Le temps commence avant l’ouverture via LaunchServices.

| Condition et mesure                             |     Vue/Tauri |          GPUI |
| ----------------------------------------------- | ------------: | ------------: |
| Cache neuf, CPU prêt observé                    | 874,7 / 916,2 | 840,0 / 857,9 |
| Cache réutilisé, CPU prêt observé               | 526,6 / 562,7 | 437,0 / 465,2 |
| Cache neuf, occasion de rendu / soumission      | 907,8 / 949,2 | 848,4 / 866,4 |
| Cache réutilisé, occasion de rendu / soumission | 556,5 / 594,7 | 443,0 / 473,4 |

L’amélioration médiane de l’état CPU prêt observé est de **4,0 % à neuf** et **17,0 % avec le cache réutilisé**. La fin du travail est observée à la cadence de chaque application, autour de 60 Hz dans le WebView et 120 Hz dans GPUI. Tauri attend ensuite deux callbacks `requestAnimationFrame`, GPUI une soumission ultérieure `Present`. Ces proxys ont des planchers différents : leurs ratios ne sont ni des facteurs d’accélération perceptible, ni une mesure physique clic→photons.

## Détails et diffs

En ms, **p50 / p95 des médianes par lancement**, CPU prêt observé.

| Condition                          |     Vue/Tauri |         GPUI |
| ---------------------------------- | ------------: | -----------: |
| Détails, cache neuf                |   85,0 / 97,0 |   8,3 / 16,7 |
| Détails, cache réutilisé           |   85,0 / 86,0 |   8,3 / 16,7 |
| Petit diff coloré, cache neuf      |   17,0 / 17,0 |    8,3 / 8,5 |
| Petit diff coloré, cache réutilisé |   17,0 / 17,0 |    8,3 / 8,4 |
| Grand diff, cache neuf             | 225,0 / 235,0 | 92,4 / 100,9 |
| Grand diff, cache réutilisé        | 225,0 / 238,0 | 92,3 / 101,3 |

### Occasion de rendu / soumission

En ms, **p50 / p95 des médianes par lancement**, rendu / soumission.

| Condition                          |     Vue/Tauri |          GPUI |
| ---------------------------------- | ------------: | ------------: |
| Détails, cache neuf                | 117,0 / 131,0 |   16,7 / 25,0 |
| Détails, cache réutilisé           | 118,0 / 118,0 |   16,7 / 25,1 |
| Petit diff coloré, cache neuf      |   50,0 / 50,0 |   16,7 / 16,8 |
| Petit diff coloré, cache réutilisé |   50,0 / 50,0 |   16,7 / 16,8 |
| Grand diff, cache neuf             | 234,0 / 251,0 | 100,8 / 109,9 |
| Grand diff, cache réutilisé        | 234,0 / 251,0 | 100,1 / 109,4 |

Les sélections utilisent les vrais gestionnaires et attendent le commit/fichier demandé. Les petits diffs colorés sont proches du plancher d’observation d’une frame. Les lectures répétées profitent des caches habituels ; elles ne décrivent pas une première ouverture indépendante à chaque interaction. La découverte du fichier dans la liste DOM virtualisée Tauri précède le timing isolé ; GPUI trouve son index directement dans le modèle. Le CPU et la durée de toute la phase de coloration incluent une navigation et des attentes différentes, donc ne mesurent pas les seuls parseurs. L’ouverture du grand diff GPUI comprend environ un callback de planification supplémentaire.

## Mémoire de l’application entière

En MiB, **p50 / p95 entre les lancements**. Tauri inclut son processus et les trois XPC WebKit attribués par PID responsable exact : WebContent, GPU et Networking. GPUI inclut le moteur et ses threads natifs. Le stable est la médiane de la phase inactive après exclusion des premières 500 ms ; le pic est le maximum simultané observé par lancement.

| Condition et mesure                        |     Vue/Tauri |          GPUI |
| ------------------------------------------ | ------------: | ------------: |
| RSS stable, cache neuf                     | 555,6 / 578,8 | 198,9 / 201,2 |
| RSS stable, cache réutilisé                | 554,4 / 570,1 | 186,8 / 187,1 |
| Pic RSS, cache neuf                        | 555,6 / 578,9 | 198,9 / 202,3 |
| Pic RSS, cache réutilisé                   | 554,4 / 570,1 | 186,9 / 187,9 |
| Empreinte physique stable, cache neuf      | 286,8 / 313,7 | 248,2 / 250,3 |
| Empreinte physique stable, cache réutilisé | 283,7 / 292,6 | 248,3 / 249,2 |
| Pic empreinte physique, cache neuf         | 547,0 / 585,0 | 252,7 / 259,7 |
| Pic empreinte physique, cache réutilisé    | 564,6 / 575,6 | 251,0 / 253,5 |

L’empreinte physique stable baisse de **12,5 à 13,5 %**, et son pic observé d’environ **54 %**. RSS et empreinte physique sont distincts ; une somme de RSS peut compter des pages partagées plusieurs fois. WindowServer et les pilotes GPU globaux ne sont pas attribués. Les prélèvements de 100 ms commencent au retour du PID LaunchServices et peuvent manquer un pic avant ce retour ou entre deux lectures.

## CPU et cadence de défilement

CPU utilisateur+système attribué, **100 % = un cœur**, **p50 / p95** des moyennes par phase et lancement. Plusieurs cœurs peuvent dépasser 100 %. Le repos représente environ 1,5 seconde utile ; ce n’est pas un test d’autonomie.

| Condition et phase          |       Vue/Tauri |            GPUI |
| --------------------------- | --------------: | --------------: |
| Historique, cache neuf      | 52,44 / 54,07 % | 42,21 / 43,02 % |
| Historique, cache réutilisé | 52,18 / 54,18 % | 42,18 / 42,74 % |
| Grand diff, cache neuf      | 27,96 / 30,37 % | 45,84 / 46,66 % |
| Grand diff, cache réutilisé | 28,37 / 29,55 % | 45,63 / 46,44 % |
| Repos, cache neuf           |   0,75 / 1,06 % |   1,91 / 2,43 % |
| Repos, cache réutilisé      |   0,74 / 1,07 % |   1,98 / 2,62 % |

La cadence native observée est proche de 120 Hz contre des callbacks WebView proches de 60 Hz. Les deux applications n’ont donc pas le même nombre d’occasions de frame pendant ces trois secondes ; leur CPU par phase doit être interprété avec cette différence. Les intervalles suivants décrivent deux proxys distincts, sans prouver un FPS écran ni l’absence de frames physiques manquées. Le scénario commande trois secondes à 2400 pixels/s, mais ne stocke pas la distance réellement parcourue. Vue rend les 2000 lignes DOM du patch ; GPUI virtualise le code. Cette optimisation participe à la comparaison des implémentations complètes.

En ms : médiane regroupée ; médiane / p95 du **p95 de chaque lancement**.

| Condition                   |             rAF Tauri |      Soumission GPUI |
| --------------------------- | --------------------: | -------------------: |
| Historique, cache neuf      | 17,00 ; 18,00 / 18,00 | 8,33 ; 10,51 / 13,24 |
| Historique, cache réutilisé | 17,00 ; 18,00 / 18,00 | 8,34 ; 10,78 / 12,39 |
| Grand diff, cache neuf      | 17,00 ; 18,00 / 18,00 | 8,33 ; 11,15 / 13,37 |
| Grand diff, cache réutilisé | 17,00 ; 18,00 / 18,00 | 8,33 ; 10,57 / 12,40 |

Le dessin CPU GPUI médian par frame est d’environ **2,9 ms dans l’historique** et **3,3 ms dans le diff**. Le JSON conserve `draw_ms` et `submit_ms`. Tauri n’expose pas ces mêmes événements publics ; aucun coût de dessin comparable n’est inventé.

## Contrôle des unités CPU

Les compteurs `proc_pid_rusage` sont convertis de ticks Mach en nanosecondes **pendant chaque prélèvement, avant les deltas CPU**. Le rapport local `mach_timebase_info` est **125/3 ns par tick**, conformément aux [tests officiels Apple XNU](https://github.com/apple-oss-distributions/xnu/blob/main/tests/recount/recount_perf_tests.c#L183-L215). Le JSON atteste `context.cpu_timebase_conversion.applied_during_sampling=true`. Aucune multiplication après collecte n’est appliquée à cette série.

Une calibration indépendante après le dernier lancement, sur **400,001 ms** murales, donne **399,681 ms** de CPU rusage contre **399,654 ms** via `time.process_time_ns()`, ratio **1,000068**, soit **99,920 % d’un cœur**. Elle ne perturbe pas les benchmarks. Les tests de statistiques/conversion valident ces unités.

## Artefacts mesurés

Les binaires et les **130 fichiers source/manifeste/harness sélectionnés** sont inchangés pendant les 120 lancements. Le contexte conserve leur méthode de hash, les hashes du helper LaunchServices source/compilé, le matériel et la calibration.

| Artefact          | Taille de l’exécutable | SHA-256                                                            |
| ----------------- | ---------------------: | ------------------------------------------------------------------ |
| Vue/Tauri release |      13 499 472 octets | `024d8b1fb388806c11885fe6e3ac7815f44bce0a288e59ab041d0bc12c7708f7` |
| GPUI release      |      65 420 384 octets | `9be0a07486be73caa30a99c1565920d04f50b08e28dfc91b2b25cc69d905c15a` |

Les tailles décrivent les exécutables, pas les bundles avec leurs ressources.

- Révision Git au début : `dbce1c713858dd02ed294eaaf85bed676f4e42d5` ; au terme : `dbce1c713858dd02ed294eaaf85bed676f4e42d5`. Les sources sélectionnées sont identifiées par hash.
- Hash des sources sélectionnées : `ca137b7e0ed0391d4e9beb4f9a3b037205bb99469e5064856067fe3a6a5c8415`.
- Runner exécuté : `8bfb0c64203e3c3e9348d555a2c096b14310bae5502664700ae68995cab1aafa`.
- Contexte gelé : `2026-09-30T23:14:34.101389+00:00`.

La géométrie CoreGraphics enregistrée au lancement est un relevé précoce susceptible de refléter l’animation d’ouverture AppKit. Le viewport de contenu de 1440 × 888 est celui réellement validé ; GPUI attend cette taille avant le démarrage prêt et la relève pendant le scénario.

## Contrôle après la série

La série de 120 lancements reste associée à la révision dbce1c7 (binaire GPUI 9be0a07486be…). Après cette série, le correctif 872d9ae refuse les résultats de coloration dont le budget de 500 ms est épuisé ; cette limite reste inchangée. Les nouveaux bundles ont passé un contrôle séparé de quatre lancements complets, avec 30 sélections de commit et 30 diffs réellement colorés par lancement, le même baseline Tauri, la même fixture et le même viewport. Leurs sources et binaires sont inchangés pendant ce contrôle. Les mesures et quantiles des 120 lancements restent inchangés ; aucun nouveau quantile de performance n’est publié. Le manifeste QA distingue les captures initiales et le contrôle natif du nouveau bundle de production.

## Contrôle de lecture sur un commit réel

Le 1er octobre 2026, le commit `cbf00ed834a68bcb935cc9e6baff219c030333cb` (145 fichiers) a été examiné sur la même machine, en release, à partir de la base `8950804`. La sonde `inspect-latency` utilise les lectures Git et la coloration de production, mais exclut le rendu, les files d’attente de l’interface et le défilement. Cinq nouveaux processus ont été lancés avant et cinq après le correctif du cache des identités de fichiers ; le cache filesystem n’était pas contrôlé et les autres applications sont restées ouvertes. Les résultats bruts locaux sont conservés dans `.mangrove-dev/performance/before-*.json` et `after-*.json` du worktree du correctif.

Avant le correctif, la médiane de lecture de chacun des six fichiers sondés était comprise entre **82,7 et 83,5 ms** : chaque patch recalculait la détection des renommages de l’ensemble du commit. Après le correctif, qui réutilise les identités résolues puis compare seulement les chemins sélectionnés, ces médianes sont comprises entre **0,6 et 1,1 ms**. La lecture initiale de la liste reste autour de **84 ms** ; la coloration reste autour de **9 à 31 ms**. Les fichiers sondés sont `.github/workflows/ci.yml`, `src-gpui/src/code_view.rs`, `file_browser.rs`, `history.rs`, `inspector.rs` et `syntax.rs`.

Ce correctif porte sur le moteur partagé : il profite à GPUI et à tout consommateur du moteur mis à jour. Le build Tauri historique reste inchangé. Ces mesures établissent un gain de lecture des patches après chargement de la liste ; elles ne mesurent pas la latence clic→affichage et ne prouvent pas une amélioration du scroll, ni une supériorité globale sur Tauri. Une inspection native du diff coloré de `code_view.rs` (1 619 lignes) a été réalisée en release ; le profil d’échantillonnage ne fournit pas les intervalles de frame nécessaires pour quantifier les saccades.

```sh
cargo run --release --locked --manifest-path src-gpui/Cargo.toml \
  --example inspect-latency -- /chemin/du/depot \
  cbf00ed834a68bcb935cc9e6baff219c030333cb \
  .github/workflows/ci.yml src-gpui/src/code_view.rs \
  src-gpui/src/file_browser.rs src-gpui/src/history.rs \
  src-gpui/src/inspector.rs src-gpui/src/syntax.rs
```

## Scroll du grand diff coloré

Un second contrôle porte sur `src-gpui/src/code_view.rs` au même commit : **1 619 lignes avec coloration**, fenêtre de contenu 1440 × 888, cinq paires de nouveaux processus release avec `app-benchmark`, cache SQLite réutilisé et ordre avant/après alterné. Les deux binaires utilisent le moteur corrigé ; seuls les caches de vue autour de la sidebar et de l’historique changent dans le renderer. LaunchServices atteste l’activation sur l’écran principal pour chaque lancement retenu. Trois lancements dont l’activation n’a pas été attestée sont exclus. Les essais exploratoires précédents sont distincts de cette série. Les [relevés par lancement, hashes et critères](performance-scroll-gpui.json) permettent de vérifier les agrégats ; les rapports de frames bruts restent locaux dans `.mangrove-dev/performance/scroll-panels-verified-*.json`.

La **médiane des médianes de dessin CPU par frame passe de 2,709 à 2,498 ms**, soit **7,8 % de moins**. La médiane des p95 passe de 3,409 à 3,248 ms. Les intervalles de soumission médians restent autour de **8,32 ms** ; la médiane de leurs p95 passe de 8,96 à 9,18 ms. Un pic de 49,5 ms existe après le changement. Cette petite série établit un coût de dessin réduit dans ce scénario, mais ne démontre ni une cadence écran améliorée, ni l’absence de saccades, ni un avantage sur Tauri. La charge de fond n’est pas neutralisée et les trois secondes à 2400 pixels/s sont une commande de scroll, pas un relevé de distance parcourue.

L’optimisation réutilise les panneaux inchangés via `Entity::cached`, avec une boîte de layout de taille définie, lorsque l’accessibilité n’est pas active. GPUI 0.3.7 ne rejoue pas les nœuds AccessKit de ces sous-arbres : une consultation native a fait disparaître l’historique de l’arbre d’accessibilité après filtrage de la sidebar. Le rendu complet est donc conservé quand `window.is_a11y_active()` est vrai ; GPUI force un redraw à l’activation de l’accessibilité. Les mesures ci-dessus décrivent la branche sans accessibilité active et précèdent cet ajout du garde. Les changements de données et de thème notifient déjà ces vues ; le cache GPUI tient aussi compte des bounds, du clipping et du style hérité. Ce choix suit le [guide officiel des caches de vue GPUI Kit](https://gpui-kit.com/docs/view-cache/). Le cache expérimental des styles de lignes a été retiré : ses cinq paires exploratoires n’ont pas montré de gain net.

Le harness accepte désormais des cibles explicites, uniquement quand `MANGROVE_BENCH_OUTPUT` active le benchmark. Pour reproduire le scénario, utiliser une copie bare du dépôt dont la seule branche pointe sur `cbf00ed834a68bcb935cc9e6baff219c030333cb`, puis :

```sh
sh src-gpui/scripts/package-macos.sh --benchmark
MANGROVE_BENCH_REPO=/chemin/absolu/fixture.git \
MANGROVE_BENCH_CACHE=/chemin/absolu/cache \
MANGROVE_BENCH_OUTPUT=/chemin/absolu/report.json \
MANGROVE_BENCH_SAMPLES=2 \
MANGROVE_BENCH_SYNTAX_0=src-gpui/src/history.rs \
MANGROVE_BENCH_SYNTAX_1=src-gpui/src/inspector.rs \
MANGROVE_BENCH_HEAVY=src-gpui/src/code_view.rs \
'src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app/Contents/MacOS/mangrove-gpui'
```

La commande directe reproduit la charge dans l’application ; pour la série contrôlée, les bundles figés avant/après sont lancés avec `scripts/benchmark-launch.swift` afin d’attester l’activation et l’écran. Le baseline reprend `dba0efb` avec les mêmes cibles de benchmark, sans les deux frontières `cached` ; leurs binaires sont identifiés dans le relevé JSON. Les cibles historiques restent les valeurs par défaut.

Le build final a passé les 77 tests de production et les 82 tests avec `app-benchmark`, Clippy sur toutes les cibles avec `-D warnings`, le formatage et le packaging release. Un lancement fonctionnel après ajout du garde valide à nouveau les 145 fichiers, les 1 619 lignes colorées et `accessibility_active=false` ; ce lancement est hors série quantitative. Le contrôle natif avec accessibilité active conserve l’historique après filtrage, restaure les références et permet de changer de commit.

## Scroll de la liste des modifications locales

La liste « Non indexés » utilise déjà `uniform_list` : seules les lignes visibles, plus une ligne de mesure, sont construites. Les lignes et le statut sont partagés par `Arc` ; un redraw ne clone pas les 5 000 fichiers. Le cache sidebar/historique introduit dans `19451ce` protège également ces panneaux pendant ce scroll. En revanche, un diff ouvert à côté était encore reconstruit sans changement. Les vues du diff central local/stash, du diff compact et de la consultation centrale d’un fichier de commit utilisent maintenant la même frontière de cache, extraite dans `view_cache.rs`, avec le même garde d’accessibilité. Les changements de fichier, de thème, de taille de texte et le scroll propre au diff notifient leurs vues ; les bounds et le clipping participent à l’invalidation.

Le contrôle du 1er octobre utilise une fixture isolée de 12 commits, **10 179 fichiers non indexés**, dont les **5 000 premiers affichés**, et un diff Rust coloré de **1 619 lignes** immobile dans le panneau central. Cinq paires de processus release avec `app-benchmark` font défiler la liste pendant trois secondes à 2400 pixels/s, dans un contenu de 1440 × 888. L’ordre est alterné, le cache SQLite réutilisé, la charge de fond non neutralisée ; aucun build ni entrée native n’intervient pendant la série. La distance réellement relevée reste entre 7100 et 7250 pixels. Le baseline reprend `19451ce` avec le même harness et le helper de cache extrait, mais conserve les trois enfants de diff sans cache. Les [résultats par lancement et hashes](performance-scroll-files-gpui.json) distinguent les deux binaires mesurés du binaire final utilisé pour le diagnostic.

La médiane des médianes de dessin CPU passe de **2,107 à 1,566 ms**, soit environ **26 % de moins** ; la médiane des p95 passe de 2,610 à 1,923 ms. **La série ne permet pas de conclure à un gain de fluidité** : les médianes des intervalles de soumission de deux lancements après sont autour de 34–41 ms, contre environ 8,3 ms pour les trois autres. La médiane des p95 d’intervalle passe de 10,49 à 33,23 ms. Tous ces lancements lents sont conservés. LaunchServices attestait l’activation au départ, pas pendant toute la phase. Trois contrôles ultérieurs avec les nouveaux compteurs relèvent respectivement 9, 55 et 10 callbacks de scroll avec une fenêtre inactive, sans fenêtre masquée ; ils montrent des interruptions de focus, sans en établir la cause ni expliquer rétroactivement chaque intervalle de la série. Le coût CPU observé baisse, mais une comparaison de cadence fiable demande un contrôle continu du focus.

Un lancement dont l’activation n’a pas pu être attestée est exclu. Une première tentative de série s’était arrêtée sur un seuil inapproprié de 100 frames ; les rapports exploratoires restent locaux. La série complète conserve aussi les lancements avec peu de frames et n’exige que deux échantillons pour calculer les statistiques. Les données brutes restent dans `.mangrove-dev/performance/local-files-complete-*.json`. Les compteurs d’activité et une réécriture équivalente du `match` demandée par Clippy sont postérieurs aux binaires comparés ; ils ne changent pas le cache de production.

Le scénario optionnel se lance sur un dépôt de test contenant au moins dix commits et des changements non indexés, avec `MANGROVE_BENCH_SCROLL_TARGET=unstaged-files` et `MANGROVE_BENCH_HEAVY` égal au chemin exact d’un fichier non indexé présent parmi les 5 000 premiers. Le fichier choisi reste ouvert pendant le scroll de la liste. Les variables ne sont lues que lorsque `MANGROVE_BENCH_OUTPUT` active le benchmark ; le parcours historique reste la valeur par défaut. Le rapport inclut les nombres total/affiché, la distance de la liste et les callbacks avec fenêtre inactive ou masquée.

```sh
MANGROVE_BENCH_REPO=/chemin/absolu/depot-test \
MANGROVE_BENCH_CACHE=/chemin/absolu/cache-test \
MANGROVE_BENCH_OUTPUT=/chemin/absolu/file-scroll.json \
MANGROVE_BENCH_SCROLL_TARGET=unstaged-files \
MANGROVE_BENCH_HEAVY=00-large.rs \
'src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app/Contents/MacOS/mangrove-gpui'
```

Le contrôle natif avec accessibilité active vérifie le défilement de cette liste, le changement de fichier avec les flèches, le passage central/compact, Home pour revenir au premier fichier et le scroll du grand diff compact. L’historique et le diff restent présents dans l’arbre accessible. Le cache est désactivé pendant ce contrôle, comme prévu par le garde. Un contrôle fonctionnel distinct du scénario historique vérifie le cache de la consultation de commit et son propre scroll ; ses timings ne sont pas ajoutés à la comparaison.

Validation du complément : 77 tests release de production, 82 avec `app-benchmark`, Clippy toutes cibles avec `-D warnings`, formatage et packaging macOS release. Les bundles de test isolés sont signés ad hoc et leur signature est vérifiée.

## Surlignage des mots modifiés

Le 10 octobre 2026, sur le même Apple M4 Pro, la sonde ignorée `changed_words_timing_probe` mesure le seul calcul des mots modifiés en release : 41 exécutions par cas, sur des lignes dont un nombre, un mot et un booléen changent. Elle exclut la lecture Git, la coloration, la publication et le rendu.

En ms, **p50 / p95**.

| Cas                                     | Mots modifiés |
| --------------------------------------- | ------------: |
| 1 000 paires (2 000 lignes), un bloc    |   1,69 / 2,01 |
| 1 000 paires, blocs de 10 paires        |   3,39 / 3,56 |
| 2 000 paires (4 000 lignes), un bloc    |   3,32 / 3,39 |
| 2 000 paires, blocs de 10 paires        |   6,93 / 7,16 |

L’objectif était de rester sous 20 ms au p95 pour 2 000 lignes appariées ; un patch affiché compte au plus 2 000 lignes. Sur trois exécutions, les p50 varient de moins de 0,3 ms et, avec la charge de fond, les p95 restent sous 5,4 ms. Les blocs de 10 paires passent par l’appariement par ressemblance, tandis qu’un bloc de plus de 1 024 combinaisons apparie ses lignes dans l’ordre. Le calcul précède la coloration dans la même tâche de fond : il retarde d’autant la publication des couleurs, pas l’affichage du patch brut.

La fixture `--large-change` modifie aussi 960 paires de lignes de `packages/core/src/search.ts`, en 48 blocs de 20 séparés par une ligne inchangée. Son patch de 1 973 lignes reste sous la borne d’affichage et le commit garde ses 46 fichiers. Sur ce fichier, `inspect-latency` relève 3,7 ms pour les mots modifiés (5 759 plages) et 57 ms pour la coloration, en un seul lancement : ce relevé n’est pas un percentile.

```sh
cargo test --release --locked --manifest-path src-gpui/Cargo.toml \
  changed_words_timing_probe -- --ignored --nocapture
```

### Côte à côte

Le benchmark applicatif accepte `MANGROVE_BENCH_DIFF_LAYOUT=split`, lu seulement quand `MANGROVE_BENCH_OUTPUT` active la mesure ; `unified` reste la valeur par défaut. Le grand diff par défaut est un ajout pur et reste unifié : la comparaison des deux dispositions cible `packages/core/src/search.ts`. Le rapport indique `heavy_diff_split`, qui vaut 1 si le diff défilé était bien côte à côte, et `draw_ms` dans la phase `diff_scroll`. Ces lancements ne passent pas par le runner de la comparaison historique, qui exige le grand diff de 2 000 lignes.

Le 10 octobre 2026, sur la fixture de 10 007 commits avec `--large-change`, cinq paires de lancements directs du bundle de mesure ont alterné les deux dispositions, avec un contenu de 1440 × 888 et un cache SQLite réutilisé par disposition. Le patch de 1 973 lignes défile sans coloration syntaxique, au-delà de la borne de 20 000 tokens, mais avec ses mots modifiés. La médiane des médianes de dessin CPU par frame passe de **2,63 ms en unifié à 3,04 ms côte à côte**, soit **1,15 fois** ; la médiane des p95 passe de 3,05 à 3,47 ms, soit 1,14 fois. L’objectif était de rester sous 1,5 fois. Les intervalles de soumission restent autour de 8,31 ms dans les deux dispositions. L’activation des fenêtres n’est pas attestée et la charge de fond n’est pas neutralisée ; les [relevés par lancement](performance-split-gpui.json) conservent le hash du binaire mesuré.

Sur le thread d’interface, construire la disposition côte à côte d’environ 2 000 lignes coûte 0,04 ms quand les paires viennent de la préparation en arrière-plan. Un patch affiché avant celle-ci est apparié sur place : 1,9 ms au p95 pour des blocs de 20 paires et 3,7 ms pour des blocs de 32 longues paires, selon la sonde ignorée `split_layout_timing_probe` en release.

```sh
for layout in unified split; do
  MANGROVE_BENCH_REPO=/chemin/absolu/fixture-10000 \
  MANGROVE_BENCH_CACHE=/chemin/absolu/cache-$layout \
  MANGROVE_BENCH_OUTPUT=/chemin/absolu/diff-$layout.json \
  MANGROVE_BENCH_HEAVY=packages/core/src/search.ts \
  MANGROVE_BENCH_DIFF_LAYOUT=$layout \
  MANGROVE_BENCH_SAMPLES=2 \
  'src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app/Contents/MacOS/mangrove-gpui'
done
```
