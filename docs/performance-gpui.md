# Résultats de la comparaison Vue/Tauri et GPUI

Le gain principal est la mémoire RSS : la médiane stable passe de **556 à 199 MiB** avec un cache neuf, et de **554 à 187 MiB** avec un cache réutilisé, soit **64 à 66 % de moins**. Les tableaux décrivent séparément la réactivité et le coût des phases : le CPU baisse dans l’historique et augmente dans le grand diff. Ces chiffres comparent les applications complètes et leurs choix de rendu, avec le même moteur Git.

Les deux artefacts sont des builds **release avec `app-benchmark` activé**, utilisant les sources de l’application GPUI production `src-gpui` et du baseline Vue/Tauri conservé. Tauri active aussi le frontend via `VITE_APP_BENCHMARK=1`. Le coût du scénario et du profiler GPUI entre dans les mesures. Les builds courants compilent ces interfaces sans les modules de mesure ; les noms et identifiants des bundles de benchmark sont distincts.

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

- Révision Git au début : `77b15d5ba5a3671ebeef8f1e8f8384429bc81e58` ; au terme : `77b15d5ba5a3671ebeef8f1e8f8384429bc81e58`. Les sources sélectionnées sont identifiées par hash.
- Hash des sources sélectionnées : `ca137b7e0ed0391d4e9beb4f9a3b037205bb99469e5064856067fe3a6a5c8415`.
- Runner exécuté : `8bfb0c64203e3c3e9348d555a2c096b14310bae5502664700ae68995cab1aafa`.
- Contexte gelé : `2026-09-30T23:14:34.101389+00:00`.

La géométrie CoreGraphics enregistrée au lancement est un relevé précoce susceptible de refléter l’animation d’ouverture AppKit. Le viewport de contenu de 1440 × 888 est celui réellement validé ; GPUI attend cette taille avant le démarrage prêt et la relève pendant le scénario.

## Contrôle après la série

La série de 120 lancements reste associée à la révision 77b15d5 (binaire GPUI 9be0a07486be…). Après cette série, le correctif bb1c900 refuse les résultats de coloration dont le budget de 500 ms est épuisé ; cette limite reste inchangée. Les nouveaux bundles ont passé un contrôle séparé de quatre lancements complets, avec 30 sélections de commit et 30 diffs réellement colorés par lancement, le même baseline Tauri, la même fixture et le même viewport. Leurs sources et binaires sont inchangés pendant ce contrôle. Les mesures et quantiles des 120 lancements restent inchangés ; aucun nouveau quantile de performance n’est publié. Le manifeste QA distingue les captures initiales et le contrôle natif du nouveau bundle de production.
