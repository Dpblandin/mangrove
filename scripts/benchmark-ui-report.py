#!/usr/bin/env python3
"""Render the historical 120-launch Vue/Tauri-GPUI comparison from its archive.

This generator reads archived measurements, not current GPUI-only runner output.
The comparative sources remain in Git history before the removal of Tauri.
"""

import argparse
import json
from pathlib import Path


def render(report):
    assert report["complete"] and report["completed_launches"] == 120
    context = report["context"]
    assert context["cpu_timebase_conversion"]["applied_during_sampling"]
    data = report["summary"]
    cold, warm = "cold_app_cache", "warm_app_cache"
    assert all(data[mode][app]["launches"] == 30
               for mode in (cold, warm) for app in ("tauri", "gpui"))

    def number(value, digits=1):
        return f"{value:.{digits}f}".replace(".", ",")

    def pair(metric, digits=1):
        return f"{number(metric['p50'], digits)} / {number(metric['p95'], digits)}"

    def timing(mode, app, key):
        return data[mode][app]["timings"][key]["per_launch_p50_ms"]

    def resource(mode, app, key, phase=None):
        values = data[mode][app]["resources"]
        return values[phase][key] if phase else values[key]

    def reduction(mode, key, phase=None):
        return 100 * (1 - resource(mode, "gpui", key, phase)["p50"]
                      / resource(mode, "tauri", key, phase)["p50"])

    def cpu_comparison(phase):
        changes = [reduction(mode, "cpu_percent_one_core", phase) for mode in (cold, warm)]
        if all(change > 0 for change in changes):
            return "baisse"
        if all(change < 0 for change in changes):
            return "augmente"
        return "varie selon le cache"

    def rows(items, fetch, digits=1, suffix=""):
        return "\n".join(f"| {label} | {pair(fetch(mode, 'tauri', key), digits)}{suffix} | "
                         f"{pair(fetch(mode, 'gpui', key), digits)}{suffix} |"
                         for label, mode, key in items)

    sections = ["# Résultats de la comparaison Vue/Tauri et GPUI"]
    sections.append("**Comparaison historique archivée.** Les 120 mesures restent associées à leur provenance d’origine : base `dbce1c713858dd02ed294eaaf85bed676f4e42d5` et hash de l’arbre source alors non committé, conservés dans le JSON. Le code comparatif complet avant retrait de Tauri est disponible à `0476530b84bbd4bb36c8592cded6b85ad65127f6`, avec des correctifs ultérieurs ; cette révision n’est pas celle de l’arbre mesuré. Le runner courant mesure GPUI seul. Les chiffres ci-dessous ne sont pas des mesures du binaire courant.")
    stage = context.get("series_stage", "")
    if "before production" in stage:
        sections.append("Cette série porte sur les corrections de fidélité **avant la migration d’architecture production**. Elle est archivée séparément ; les chiffres de publication devront porter les artefacts production finaux.")
    sections.append(f"Le gain principal est la mémoire RSS : la médiane stable passe de **{number(resource(cold, 'tauri', 'resident_mib', 'idle')['p50'], 0)} à {number(resource(cold, 'gpui', 'resident_mib', 'idle')['p50'], 0)} MiB** avec un cache neuf, et de **{number(resource(warm, 'tauri', 'resident_mib', 'idle')['p50'], 0)} à {number(resource(warm, 'gpui', 'resident_mib', 'idle')['p50'], 0)} MiB** avec un cache réutilisé, soit **{number(reduction(cold, 'resident_mib', 'idle'), 0)} à {number(reduction(warm, 'resident_mib', 'idle'), 0)} % de moins**. Les tableaux décrivent séparément la réactivité et le coût des phases : le CPU {cpu_comparison('history_scroll')} dans l’historique et {cpu_comparison('diff_scroll')} dans le grand diff. Ces chiffres comparent les applications complètes et leurs choix de rendu, avec le même moteur Git.")
    if context.get("build_configuration"):
        sections.append("Les deux artefacts mesurés étaient des builds **release avec `app-benchmark` activé**, utilisant les sources GPUI production `src-gpui` et le baseline Vue/Tauri désormais archivé. Tauri activait aussi le frontend via `VITE_APP_BENCHMARK=1`. Le coût du scénario et du profiler GPUI entre dans ces mesures. Les builds ordinaires de cette révision compilaient les interfaces sans les modules de mesure ; les noms et identifiants des bundles de benchmark étaient distincts.")
    if context.get("shared_engine_dependencies"):
        versions = ", ".join(f"`{name} {version}`" for name, version
                             in context["shared_engine_dependencies"].items())
        sections.append("Les deux familles de lockfiles utilisent les mêmes versions des dépendances du moteur partagé : " + versions + ".")
    sections.append("## Série et environnement\n\nLes [données complètes](performance-gpui.json) contiennent **120 lancements réussis**, 30 par application et condition de cache. Chaque lancement réalise 30 sélections de commit, 30 petits diffs effectivement colorés, trois secondes de défilement du grand diff, trois secondes de défilement de l’historique et deux secondes au repos. Cela représente 3600 sélections de détails et 3600 sélections de diff coloré.\n\n- Apple M4 Pro, 48 GiB de RAM, macOS 26.6.2, alimentation secteur. Applications habituelles laissées ouvertes ; charge de fond non neutralisée. Aucun build ni intervention sur les fenêtres mesurées.\n- Écran principal interne : 1512 × 982 pixels logiques, 3024 × 1964 physiques, échelle 2, fréquence déclarée de 120 Hz. Deux écrans externes connectés ; chaque lancement est attesté actif et entièrement sur le principal.\n- Fenêtre : 1440 × 920 pixels logiques, contenu réel validé de 1440 × 888, thème Classique et polices par défaut.\n- Fixture synthétique propre : 10 007 commits, 46 fichiers modifiés dans le commit inspecté, grand patch limité par le moteur à 2000 lignes. Ce grand patch revient au texte simple des deux côtés ; les petits diffs mesurés produisent des tokens colorés.\n- Nouvelle instance activée via l’API publique LaunchServices/NSWorkspace, PID exact retourné, ordre des applications alterné. Le workload s’exécute dans les applications ; aucun clic, clavier ou navigateur externe ne le pilote.\n- Cache neuf = SQLite propre à chaque application et paire ; cache réutilisé = seconde exécution de la paire. Le cache filesystem reste chaud ou incontrôlé ; les processus recréent leurs caches mémoire et workers.\n\nLes quantiles utilisent le rang le plus proche. Pour les opérations répétées, les p50/p95 ci-dessous décrivent la distribution des **médianes par lancement**, soit 30 unités indépendantes par condition, et non le p95 de toutes les interactions regroupées. Ces dernières restent disponibles dans le JSON. Le [protocole](performance-gpui-protocol.md) précise les critères, les caches et la reproduction.")
    startup = [("Cache neuf, CPU prêt observé", cold, "startup_history_cpu_ready"),
               ("Cache réutilisé, CPU prêt observé", warm, "startup_history_cpu_ready"),
               ("Cache neuf, occasion de rendu / soumission", cold, "startup_history"),
               ("Cache réutilisé, occasion de rendu / soumission", warm, "startup_history")]
    sections.append("## Démarrage jusqu’à la vue utile\n\nEn ms, **p50 / p95**. La vue utile exige dix lignes visibles chargées, le graphe, les détails du premier commit et le bon viewport. Le temps commence avant l’ouverture via LaunchServices.\n\n| Condition et mesure | Vue/Tauri | GPUI |\n| --- | ---: | ---: |\n" + rows(startup, timing))
    startup_gains = [100 * (1 - timing(mode, "gpui", "startup_history_cpu_ready")["p50"]
                           / timing(mode, "tauri", "startup_history_cpu_ready")["p50"])
                     for mode in (cold, warm)]
    sections.append(f"L’amélioration médiane de l’état CPU prêt observé est de **{number(startup_gains[0])} % à neuf** et **{number(startup_gains[1])} % avec le cache réutilisé**. La fin du travail est observée à la cadence de chaque application, autour de 60 Hz dans le WebView et 120 Hz dans GPUI. Tauri attend ensuite deux callbacks `requestAnimationFrame`, GPUI une soumission ultérieure `Present`. Ces proxys ont des planchers différents : leurs ratios ne sont ni des facteurs d’accélération perceptible, ni une mesure physique clic→photons.")
    for ready in (True, False):
        suffix = "_cpu_ready" if ready else ""
        items = [(f"{label}, {tag}", mode, key + suffix)
                 for label, key in (("Détails", "select_details"),
                                    ("Petit diff coloré", "select_colored_diff"),
                                    ("Grand diff", "open_heavy_diff"))
                 for mode, tag in ((cold, "cache neuf"), (warm, "cache réutilisé"))]
        title = "## Détails et diffs" if ready else "### Occasion de rendu / soumission"
        measurement = "CPU prêt observé" if ready else "rendu / soumission"
        sections.append(title + "\n\nEn ms, **p50 / p95 des médianes par lancement**, " + measurement + ".\n\n| Condition | Vue/Tauri | GPUI |\n| --- | ---: | ---: |\n" + rows(items, timing))
    sections.append("Les sélections utilisent les vrais gestionnaires et attendent le commit/fichier demandé. Les petits diffs colorés sont proches du plancher d’observation d’une frame. Les lectures répétées profitent des caches habituels ; elles ne décrivent pas une première ouverture indépendante à chaque interaction. La découverte du fichier dans la liste DOM virtualisée Tauri précède le timing isolé ; GPUI trouve son index directement dans le modèle. Le CPU et la durée de toute la phase de coloration incluent une navigation et des attentes différentes, donc ne mesurent pas les seuls parseurs. L’ouverture du grand diff GPUI comprend environ un callback de planification supplémentaire.")
    memory_rows = []
    for label, key, phase in (("RSS stable", "resident_mib", "idle"),
                              ("Pic RSS", "peak_resident_mib", None),
                              ("Empreinte physique stable", "physical_footprint_mib", "idle"),
                              ("Pic empreinte physique", "peak_physical_footprint_mib", None)):
        for mode, tag in ((cold, "cache neuf"), (warm, "cache réutilisé")):
            a, b = [resource(mode, app, key, phase) for app in ("tauri", "gpui")]
            memory_rows.append(f"| {label}, {tag} | {pair(a)} | {pair(b)} |")
    sections.append("## Mémoire de l’application entière\n\nEn MiB, **p50 / p95 entre les lancements**. Tauri inclut son processus et les trois XPC WebKit attribués par PID responsable exact : WebContent, GPU et Networking. GPUI inclut le moteur et ses threads natifs. Le stable est la médiane de la phase inactive après exclusion des premières 500 ms ; le pic est le maximum simultané observé par lancement.\n\n| Condition et mesure | Vue/Tauri | GPUI |\n| --- | ---: | ---: |\n" + "\n".join(memory_rows))
    footprint_gains = [reduction(mode, "physical_footprint_mib", "idle") for mode in (cold, warm)]
    sections.append(f"L’empreinte physique stable baisse de **{number(min(footprint_gains))} à {number(max(footprint_gains))} %**, et son pic observé d’environ **{number(reduction(cold, 'peak_physical_footprint_mib'), 0)} %**. RSS et empreinte physique sont distincts ; une somme de RSS peut compter des pages partagées plusieurs fois. WindowServer et les pilotes GPU globaux ne sont pas attribués. Les prélèvements de 100 ms commencent au retour du PID LaunchServices et peuvent manquer un pic avant ce retour ou entre deux lectures.")
    cpu_items = [(f"{label}, {tag}", mode, phase)
                 for label, phase in (("Historique", "history_scroll"), ("Grand diff", "diff_scroll"), ("Repos", "idle"))
                 for mode, tag in ((cold, "cache neuf"), (warm, "cache réutilisé"))]
    sections.append("## CPU et cadence de défilement\n\nCPU utilisateur+système attribué, **100 % = un cœur**, **p50 / p95** des moyennes par phase et lancement. Plusieurs cœurs peuvent dépasser 100 %. Le repos représente environ 1,5 seconde utile ; ce n’est pas un test d’autonomie.\n\n| Condition et phase | Vue/Tauri | GPUI |\n| --- | ---: | ---: |\n" + rows(cpu_items, lambda mode, app, phase: resource(mode, app, "cpu_percent_one_core", phase), digits=2, suffix=" %"))
    sections.append("La cadence native observée est proche de 120 Hz contre des callbacks WebView proches de 60 Hz. Les deux applications n’ont donc pas le même nombre d’occasions de frame pendant ces trois secondes ; leur CPU par phase doit être interprété avec cette différence. Les intervalles suivants décrivent deux proxys distincts, sans prouver un FPS écran ni l’absence de frames physiques manquées. Le scénario commande trois secondes à 2400 pixels/s, mais ne stocke pas la distance réellement parcourue. Vue rend les 2000 lignes DOM du patch ; GPUI virtualise le code. Cette optimisation participe à la comparaison des implémentations complètes.")
    frame_rows = []
    for label, phase in (("Historique", "history_scroll"), ("Grand diff", "diff_scroll")):
        for mode, tag in ((cold, "cache neuf"), (warm, "cache réutilisé")):
            cells = []
            for app in ("tauri", "gpui"):
                metric = data[mode][app]["frames"][phase]["interval_ms"]
                cells.append(number(metric["pooled_ms"]["p50"], 2) + " ; " + pair(metric["per_launch_p95_ms"], 2))
            frame_rows.append(f"| {label}, {tag} | {' | '.join(cells)} |")
    sections.append("En ms : médiane regroupée ; médiane / p95 du **p95 de chaque lancement**.\n\n| Condition | rAF Tauri | Soumission GPUI |\n| --- | ---: | ---: |\n" + "\n".join(frame_rows))
    draw_history = data[cold]["gpui"]["frames"]["history_scroll"]["draw_ms"]["pooled_ms"]["p50"]
    draw_diff = data[cold]["gpui"]["frames"]["diff_scroll"]["draw_ms"]["pooled_ms"]["p50"]
    sections.append(f"Le dessin CPU GPUI médian par frame est d’environ **{number(draw_history)} ms dans l’historique** et **{number(draw_diff)} ms dans le diff**. Le JSON conserve `draw_ms` et `submit_ms`. Tauri n’expose pas ces mêmes événements publics ; aucun coût de dessin comparable n’est inventé.")
    cal = context["cpu_calibration"]
    conversion = context["cpu_timebase_conversion"]
    sections.append(f"## Contrôle des unités CPU\n\nLes compteurs `proc_pid_rusage` sont convertis de ticks Mach en nanosecondes **pendant chaque prélèvement, avant les deltas CPU**. Le rapport local `mach_timebase_info` est **{conversion['numer']}/{conversion['denom']} ns par tick**, conformément aux [tests officiels Apple XNU](https://github.com/apple-oss-distributions/xnu/blob/main/tests/recount/recount_perf_tests.c#L183-L215). Le JSON atteste `context.cpu_timebase_conversion.applied_during_sampling=true`. Aucune multiplication après collecte n’est appliquée à cette série.\n\nUne calibration indépendante après le dernier lancement, sur **{number(cal['observed_wall_ms'], 3)} ms** murales, donne **{number(cal['rusage_converted_cpu_ms'], 3)} ms** de CPU rusage contre **{number(cal['independent_process_time_ms'], 3)} ms** via `time.process_time_ns()`, ratio **{number(cal['rusage_to_process_time_ratio'], 6)}**, soit **{number(cal['cpu_percent_one_core'], 3)} % d’un cœur**. Elle ne perturbe pas les benchmarks. Les tests de statistiques/conversion valident ces unités.")
    provenance = context["source_provenance"]
    artifact_rows = []
    for app, label in (("tauri", "Vue/Tauri release"), ("gpui", "GPUI release")):
        artifact = context["artifacts"][app]
        size = f"{artifact['bytes']:,}".replace(",", " ")
        artifact_rows.append(f"| {label} | {size} octets | `{artifact['sha256']}` |")
    sections.append(f"## Artefacts mesurés\n\nLes binaires et les **{provenance['source_files_hashed']} fichiers source/manifeste/harness sélectionnés** sont inchangés pendant les 120 lancements. Le contexte conserve leur méthode de hash, les hashes du helper LaunchServices source/compilé, le matériel et la calibration.\n\n| Artefact | Taille de l’exécutable | SHA-256 |\n| --- | ---: | --- |\n" + "\n".join(artifact_rows) + f"\n\nLes tailles décrivent les exécutables, pas les bundles avec leurs ressources.\n\n- Révision Git au début : `{provenance.get('git_revision_at_start', provenance['base_git_revision'])}` ; au terme : `{provenance.get('git_revision_at_end', provenance['base_git_revision'])}`. Les sources sélectionnées sont identifiées par hash.\n- Hash des sources sélectionnées : `{provenance['uncommitted_source_tree_sha256']}`.\n- Runner exécuté : `{context['runner_source_sha256']}`.\n- Contexte gelé : `{provenance['captured_at_utc']}`.\n\nLa géométrie CoreGraphics enregistrée au lancement est un relevé précoce susceptible de refléter l’animation d’ouverture AppKit. Le viewport de contenu de 1440 × 888 est celui réellement validé ; GPUI attend cette taille avant le démarrage prêt et la relève pendant le scénario.")
    follow_up = context.get("post_series_validation", {}).get("note")
    if follow_up:
        sections.append("## Contrôle après la série\n\n" + follow_up)
    return "\n\n".join(sections) + "\n"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.write_text(render(json.loads(args.input.read_text())))
