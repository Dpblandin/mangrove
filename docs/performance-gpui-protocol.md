# Comparaison des interfaces Tauri et GPUI

Le benchmark lance les deux applications natives compilées en **release avec la fonctionnalité de mesure `app-benchmark`**, avec le même dépôt synthétique, la même fenêtre native de 1440 × 920 pixels logiques (zone de contenu de 1440 × 888), le thème Classique, les tailles de police par défaut et le même moteur `mangrove-core`. GPUI utilise les sources de l’application production dans `src-gpui` ; Vue/Tauri reste le baseline conservé. Les scénarios s’exécutent dans les applications elles-mêmes. Le script externe lance les processus et observe leurs ressources ; il ne pilote ni souris, ni clavier, ni navigateur externe.

Les builds courants n’activent pas `app-benchmark` : les modules de scénario, les commandes Tauri d’écriture et le profiler GPUI ne sont pas compilés. Le frontend Tauri de mesure exige aussi `VITE_APP_BENCHMARK=1`. La mesure porte donc sur les builds release instrumentés des mêmes interfaces ; leur coût d’instrumentation entre dans les chiffres. Les bundles de mesure portent des noms et identifiants distincts pour protéger les préférences habituelles.

Sur macOS, le lanceur Swift utilise l’API publique moderne `NSWorkspace.openApplication`, demande une nouvelle instance activée et transmet les arguments et variables du scénario. Il publie immédiatement le PID exact retourné par LaunchServices, puis maintient son runloop jusqu’à l’activation de l’application et à la présence d’une fenêtre entièrement sur l’écran principal. Les deux interfaces sont placées au centre de cet écran ; le rapport conserve les bornes de fenêtre, résolution logique/physique et fréquence déclarée par CoreGraphics. Le helper est compilé une seule fois avant les mesures. Aucun clic d’activation externe ne participe au scénario.

Le portage doit être fonctionnel avant de publier les résultats. Les chiffres du moteur Rust seul dans `performance.md` ne permettent pas de conclure sur le rendu de l’application.

Cette comparaison porte sur les deux implémentations complètes. La Vue actuelle crée les 2000 lignes DOM du grand diff, tandis que le portage GPUI virtualise ces lignes : un gain peut venir de cette optimisation et ne constitue pas une mesure isolée du framework.

## Scénario identique

Chaque lancement effectue les étapes suivantes :

1. Ouvrir le dépôt, attendre au moins dix lignes d’historique chargées, le graphe et les détails du premier commit, puis une occasion de rendu.
2. Attendre la fin de l’indexation.
3. Alterner trente fois entre les commits aux positions 1 et 0, via les vrais gestionnaires de sélection, jusqu’à l’affichage des détails du commit demandé.
4. Alterner trente fois entre deux petits diffs TypeScript du commit à 46 fichiers, jusqu’à la publication et à l’affichage de tokens colorés. Le chargement des fichiers, l’IPC, le worker Shiki ou le parseur natif et les caches de l’application participent à la mesure.
5. Ouvrir le diff TypeScript synthétique dont le fichier comporte 2401 lignes : le moteur partagé limite le patch affiché à 2000 lignes, avec un avertissement de troncature. Attendre sa préparation et défiler pendant trois secondes à 2400 pixels/s. La coloration des 2000 lignes est éligible selon le budget de lignes ; le budget de 20 000 tokens peut toutefois imposer un retour au texte simple. Le rapport conserve `heavy_diff_colored` et rejette les paires dont le résultat de coloration diffère. Cette charge est distincte de la mesure précédente, qui exige des petits diffs effectivement colorés.
6. Revenir à l’historique, puis défiler pendant trois secondes à la même vitesse. Les requêtes de pages et la virtualisation réelles restent actives.
7. Rester inactif deux secondes sans animation de benchmark. La mesure mémoire stable et le CPU au repos excluent les premières 500 ms de cette période.

Les deux cibles de coloration et le grand fichier sont des données synthétiques connues du protocole. Les rapports n’exportent ni chemins de dépôt/fichier, ni SHA, ni références, ni auteurs, ni contenu.

## Répétitions et caches

Par défaut, le script effectue **30 lancements par application et par condition de cache**, soit 120 lancements. Dans chaque paire, une application reçoit son propre cache SQLite neuf ; sa seconde exécution réutilise ce cache. Les caches des deux implémentations restent séparés. L’ordre Tauri/GPUI alterne entre les paires pour réduire le biais de chauffe ou de charge de la machine.

« Cache froid » désigne uniquement le **cache applicatif neuf**. Le cache filesystem de macOS/Linux reste chaud ou incontrôlé ; le protocole n’annonce aucun démarrage avec cache OS froid. Chaque lancement recommence avec une fenêtre neuve et des caches mémoire/worker neufs. Les lectures répétées dans une même fenêtre réutilisent les caches habituels de cette application.

Les quantiles utilisent la méthode du rang le plus proche : p95 = valeur au rang `ceil(0,95 × N)`. Pour les opérations répétées, le rapport conserve à la fois les échantillons regroupés et les médianes par lancement. Les 30 interactions dans un même processus sont corrélées ; elles ne représentent pas 30 démarrages indépendants.

## Ce que les timings mesurent

Le temps de démarrage inclut le lancement du processus, les bibliothèques natives, la création de fenêtre/WebView, le chargement JS ou des composants natifs, l’ouverture du dépôt et l’affichage utile défini ci-dessus. L’horloge du lancement est transmise par le script ; son rapprochement avec l’horloge monotone de l’application utilise l’horloge système au début du lancement. Un changement de l’horloge système pendant les mesures invalide la série.

Chaque opération fournit aussi une mesure `*_cpu_ready` : l’état ou le DOM est observé prêt à la cadence des callbacks de frames, avant l’occasion de rendu finale. Cette observation est quantifiée par la cadence de chaque interface ; elle ne donne pas l’instant exact où le travail asynchrone s’est terminé.

- **Tauri** attend la mise à jour Vue puis deux callbacks `requestAnimationFrame`. Ces callbacks constituent une occasion de rendu du WebView, pas la preuve que les pixels ont été affichés par l’écran. Les intervalles de défilement sont ceux des callbacks rAF.
- **GPUI** attend un événement `Present` observé après que l’état est devenu prêt. Les événements publics du profiler `gpui-pre 0.3.7` distinguent le dessin CPU et la soumission de la frame à la plateforme. Les intervalles de défilement sont ceux des soumissions de frames nouvellement dessinées.

Ces deux proxys couvrent davantage que le simple montage JS ou le retour d’un appel Rust, mais **ne sont pas équivalents à une mesure physique clic→photons**. On peut comparer la latence pratique avec cette réserve ; on ne peut pas annoncer un gain de scanout ou de latence GPU d’après ces seuls chiffres. La comparaison doit présenter séparément CPU prêt, occasion de rendu/soumission, et intervalles de frames. Pour une conclusion sur la présentation physique, compléter avec Instruments/Core Animation/Metal ou une mesure écran externe.

Les deux callbacks rAF de Tauri et la seule soumission ultérieure attendue par GPUI introduisent des planchers de cadence différents. Un rapport entre leurs timings de rendu ne constitue donc pas un facteur d’accélération perceptible. Les timings CPU prêt restent eux aussi quantifiés par leurs cadences respectives.

Pour trouver un fichier virtualisé, Tauri défile dans la liste DOM avant de commencer le timing isolé de sélection ; GPUI retrouve directement son index dans le modèle. Les lectures répétées chronométrées demandent les mêmes cibles, mais la durée et le CPU de toute la phase `syntax` incluent une navigation et des attentes différentes. Ces chiffres de phase ne mesurent pas la vitesse du seul parseur de coloration ; les attentes peuvent aussi laisser davantage de temps aux préchargements Tauri. L’ouverture du grand diff GPUI comprend environ un callback de planification supplémentaire avant l’action : conserver cette mesure séparée des autres opérations.

Le scénario commande trois secondes à 2400 pixels/s dans les deux applications. Le rapport conserve cette commande et les intervalles observés, mais ne stocke pas la distance réellement parcourue : il ne prouve pas une distance physique égale.

La fonction `profiler` de GPUI est nécessaire aux événements `Draw`/`Present`. Le tracing est activé uniquement pendant le scénario et sa mémoire entre dans le résultat ; les timings de tâches et leurs chemins source ne sont pas exportés. La mesure Tauri ne dispose pas d’un équivalent public des durées de dessin CPU ou de soumission : ces tableaux restent vides, sans chiffre inventé.

## Mémoire et CPU de l’application entière

Sur macOS, la mesure inclut le processus de l’application **et tous les processus XPC dont le PID responsable est celui de ce lancement**. Cela couvre WebContent, GPU et Networking du WebView Tauri, même lorsque leur PPID est `launchd`. La simple présence de `WebKit` dans un nom de processus ne sert jamais à lui attribuer sa mémoire.

Le script vérifie que le PID responsable de la nouvelle instance retournée par LaunchServices est son propre PID avec `responsibility_get_pid_responsible_for_pid`, puis utilise cette même API pour attribuer les XPC. Cette API macOS est non documentée et vérifiée au lancement ; son absence ou une attribution invalide fait échouer la mesure au lieu d’appliquer une estimation trompeuse. Sur Linux, le lecteur utilise les descendants du processus et `/proc` ; cette branche doit être validée sur une machine Linux avant publication.

`proc_pid_rusage` fournit les temps CPU utilisateur+système cumulés en **ticks Mach**, la mémoire résidente et l’empreinte physique. Les temps CPU sont convertis en nanosecondes avec `mach_timebase_info` pendant chaque prélèvement, avant le calcul des deltas ; aucune multiplication après collecte n’est nécessaire. Sur cette machine Apple Silicon, le rapport exact est `125/3` ns par tick. C’est la conversion appliquée dans les [tests officiels Apple XNU](https://github.com/apple-oss-distributions/xnu/blob/main/tests/recount/recount_perf_tests.c#L183-L215). Le CPU est calculé par différence sur chaque phase : **100 % = un cœur saturé**, et plusieurs cœurs peuvent dépasser 100 %. Il ne s’agit pas du pourcentage CPU lissé de `ps`.

Les ressources sont échantillonnées toutes les 100 ms par défaut. Le pic correspond à la somme simultanée des processus attribués ; un pic entre deux lectures peut être manqué. La somme des RSS peut compter des pages partagées plusieurs fois ; l’empreinte physique macOS apporte une autre perspective, sans la confondre avec le RSS. WindowServer, le pilote GPU, la mémoire système partagée et la consommation énergétique globale ne sont pas attribués à une application. GPUI inclut le moteur et ses threads dans son processus natif.

L’observation commence dès que LaunchServices retourne le PID, avant la fin de l’activation. Elle ne couvre pas les premiers instants du processus avant ce retour : le pic est un pic **observé**, pas une garantie de maximum absolu. Les timings de démarrage incluent néanmoins l’intégralité du lancement du helper et de l’application. Les processus lancés par LaunchServices ne sont pas enfants directs du runner : leur code de sortie n’est pas disponible. Le runner exige leur disparition, un rapport atomique de statut `ok`, l’activation attestée et le scénario complet, puis nettoie uniquement les processus du PID responsable de ce lancement. Un timeout du helper termine seulement la nouvelle instance qu’il a lui-même ouverte.

Les résultats bruts restent bornés à 2000 frames par phase et 100 interactions par lancement. Ils sont écrits uniquement dans le chemin de sortie explicite, avec des caches temporaires distincts. Le benchmark Tauri sauvegarde les préférences locales, applique les valeurs par défaut puis restaure les préférences avant de quitter. Un arrêt forcé peut interrompre cette restauration ; exécuter les mesures dans un bundle dédié et ne pas lancer simultanément un autre exemplaire du même bundle.

## Reproduire

Préparer la fixture avec son grand changement de 46 fichiers et compiler les deux exécutables release de mesure. Sur macOS, le packaging GPUI `--benchmark` active `app-benchmark`, utilise un répertoire `target/benchmark` distinct et produit le bundle `Mangrove GPUI Benchmark.app` (`dev.mangrove.gpui.benchmark`). L’override Tauri `src-tauri/benchmark/tauri.conf.json` conserve l’interface et crée `Mangrove Tauri Benchmark.app` (`dev.mangrove.desktop.benchmark`). Il faut activer le frontend **et** la fonctionnalité Cargo Tauri dans la même commande.

```sh
vp run fixture --commits 10000 --large-change
sh src-gpui/scripts/package-macos.sh --benchmark
VITE_APP_BENCHMARK=1 vp exec tauri build \
  --features app-benchmark --bundles app \
  --config src-tauri/benchmark/tauri.conf.json
python3 -B scripts/benchmark-ui.test.py
python3 -B scripts/benchmark-ui.py \
  --tauri 'target/release/bundle/macos/Mangrove Tauri Benchmark.app/Contents/MacOS/mangrove-desktop' \
  --gpui 'src-gpui/target/benchmark/release/bundle/Mangrove GPUI Benchmark.app/Contents/MacOS/mangrove-gpui' \
  --repository .mangrove-dev/fixture-10000 \
  --samples 30 --interaction-samples 30 \
  --output /chemin/explicite/performance-gpui.json
```

Un essai de protocole peut utiliser `--samples 1 --interaction-samples 2` ; il vérifie le scénario, pas une conclusion de performance. La sortie est enregistrée après chaque lancement terminé, même si une exécution ultérieure échoue. Conserver le contexte matériel, la fréquence/échelle de l’écran, l’alimentation et les révisions exactes des deux builds avec les résultats. Fermer les applications lourdes, attendre la fin des compilations et garder la même fenêtre active sur le même écran pendant la série.

Avant la première paire, figer les SHA-256 et tailles des exécutables **à l’intérieur des bundles**, les flags de build et les sources/manifests/lockfiles sélectionnés. Pour la série production, la provenance doit couvrir `src/`, `src-tauri/`, `crates/mangrove-core/`, `src-gpui/src/`, `src-gpui/assets/`, `src-gpui/scripts/`, les scripts de benchmark et de fixture, les manifests/lockfiles racine et GPUI, ainsi que la configuration Vite qui produit le baseline. Les fichiers `target`, `dist`, caches et rapports sont exclus. Le hash composé trie les chemins relatifs puis concatène chaque chemin UTF-8, NUL, ses octets, NUL. Vérifier les mêmes fichiers et binaires après le 120e lancement, avant toute modification ou reconstruction.

Après la série, conserver `context.hardware`, `context.source_provenance`, les hashes du runner/helper et les artefacts, puis attester `context.cpu_timebase_conversion = {applied_during_sampling: true, numer: 125, denom: 3, source: "mach_timebase_info"}` avec les valeurs réelles du lecteur. Une calibration indépendante bornée à environ 400 ms, effectuée seulement après le dernier lancement, compare le CPU rusage converti à `time.process_time_ns()` et au temps mural ; ses résultats sont enregistrés dans `context.cpu_calibration`. Les scalaires collectés ne sont pas remis à l’échelle. Le Markdown se régénère ensuite exclusivement depuis ce JSON validé :

```sh
python3 -B scripts/benchmark-ui-report.py \
  docs/performance-gpui.json docs/performance-gpui.md
python3 -B scripts/report-gpui.py
# Mise en forme des artefacts pour les contrôles du dépôt :
vp fmt docs/performance-gpui.json docs/performance-gpui.md docs/performance-gpui.html
```

Le générateur exige 120 lancements complets et le marqueur de conversion pendant sampling. Le JSON brut conserve les timings et frames ; le Markdown utilise les quantiles déjà calculés, et reporte les hashes exacts du contexte. Les anciennes séries avant migration restent archivées localement et ne sont pas fusionnées avec la série production.

Dans les builds `app-benchmark`, les variables `MANGROVE_BENCH_OUTPUT`, `MANGROVE_BENCH_CACHE`, `MANGROVE_BENCH_REPO` et `MANGROVE_BENCH_SAMPLES` activent le scénario ; le script les prépare automatiquement. Il fixe aussi `MANGROVE_GPUI_PREFERENCES` à un fichier temporaire propre au lancement pour que les entrées récentes ne modifient pas le profil habituel. Les builds courants ignorent `MANGROVE_BENCH_*`. Sans output explicite dans un build instrumenté, aucune action de benchmark ne s’exécute et les commandes natives d’écriture refusent les appels. Les chemins de cache et de sortie doivent être absolus ; le frontend ne peut ni choisir un autre fichier de sortie, ni déclencher un benchmark via une URL.

Avant d’ouvrir la fixture, le mode Tauri place sa fenêtre sur l’écran principal, puis montre et focalise explicitement sa fenêtre et son WebView via les API natives publiques. GPUI utilise son écran primaire et fixe la taille du contenu par `Window::resize` ; sa readiness de démarrage attend le viewport réel de 1440 × 888. Le helper lance et active les deux applications de la même façon. Sa géométrie CoreGraphics est un relevé précoce qui peut encore refléter l’animation d’ouverture AppKit ; les dimensions utiles sont celles du viewport réel de chaque application. Un watchdog mural rejette une attente rAF suspendue au lieu de fabriquer des frames ou de remplacer silencieusement la cadence par un timer. Un rapport d’échec peut conserver seulement des compteurs DOM et des booléens de visibilité/focus pour diagnostiquer ce cas.
