#!/usr/bin/env python3
"""Generate the standalone HTML comparison from completed measurements and QA.

Defaults are relative to this repository, regardless of the current directory.
The script writes only its explicit output and never updates measurements or QA.
"""

import argparse
import base64
from datetime import datetime
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
CAPTURES = {
    **{name: f"full-{name}" for name in
       ("history", "diff", "welcome", "mangrove", "macchiato", "blame")},
    **{f"tauri-{name}": f"tauri-{name}" for name in ("history", "diff", "welcome")},
}


def jpeg_dimensions(content):
    """Read JPEG SOF dimensions without a rendering/image-library dependency."""
    if not content.startswith(b"\xff\xd8"):
        raise ValueError("A QA capture is not a JPEG image")
    position = 2
    while position < len(content):
        if content[position] != 0xff:
            break
        while position < len(content) and content[position] == 0xff:
            position += 1
        if position >= len(content):
            break
        marker = content[position]
        position += 1
        if marker in (0xd9, 0xda):
            break
        if marker == 0x01 or 0xd0 <= marker <= 0xd8:
            continue
        if position + 2 > len(content):
            break
        size = int.from_bytes(content[position:position + 2], "big")
        if size < 2 or position + size > len(content):
            break
        if marker in (0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7,
                      0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf):
            if size < 8:
                break
            height = int.from_bytes(content[position + 3:position + 5], "big")
            width = int.from_bytes(content[position + 5:position + 7], "big")
            if width and height:
                return width, height
            break
        position += size
    raise ValueError("A QA capture has no valid JPEG dimensions")


def declared_dimensions(value):
    # Existing manifests contain sips output; new manifests can store two ints.
    if isinstance(value, list) and len(value) == 2:
        if all(type(item) is int and item > 0 for item in value):
            return tuple(value)
        if all(isinstance(item, str) for item in value):
            fields = dict(item.strip().split(":", 1) for item in value if ":" in item)
            try:
                dimensions = (int(fields["pixelWidth"]), int(fields["pixelHeight"]))
                if all(item > 0 for item in dimensions):
                    return dimensions
            except (KeyError, ValueError):
                pass
    raise ValueError("QA dimensions must contain positive width and height")


def optional_count(manifest, name):
    value = manifest.get(name)
    if value is not None and (type(value) is not int or value <= 0):
        raise ValueError(f"Invalid QA test count: {name}")
    return value


def report_data(report, manifest, screenshots):
    if not report.get("complete") or report.get("completed_launches") != 120:
        raise ValueError("Refusing an incomplete 120-launch comparison")
    if report.get("expected_launches") != 120:
        raise ValueError("The report does not describe the required 120 launches")
    context = report["context"]
    timebase = context.get("cpu_timebase_conversion", {})
    normalized = timebase.get("applied_during_sampling")
    if not normalized:
        timebase = context.get("cpu_timebase_normalization", {})
        normalized = timebase.get("applied")
    if not normalized or any(type(timebase.get(key)) is not int or timebase[key] <= 0
                             for key in ("numer", "denom")):
        raise ValueError("Refusing CPU values before Mach timebase normalization")
    if context.get("launch_samples_per_condition") != 30:
        raise ValueError("The report must contain 30 launches per application/condition")
    interactions = context.get("interaction_samples_per_launch")
    if type(interactions) is not int or not 2 <= interactions <= 100:
        raise ValueError("The report has an invalid interaction count")
    interactions = interactions // 2 * 2
    for mode in ("cold_app_cache", "warm_app_cache"):
        for application in ("tauri", "gpui"):
            if report["summary"][mode][application].get("launches") != 30:
                raise ValueError("The comparison does not contain 30 launches in each group")
    verified_at = manifest.get("verified_at")
    if not isinstance(verified_at, str):
        raise ValueError("The QA manifest has no verification timestamp")
    datetime.fromisoformat(verified_at)
    counts = []
    for field, label in (
        ("gpui_test_count", "tests GPUI, configuration normale"),
        ("gpui_benchmark_test_count", "tests GPUI, configuration instrumentée"),
        ("core_test_count", "tests du moteur Git"),
        ("vue_test_count", "tests Vue"),
        ("fixture_test_count", "tests de fixture"),
        ("linux_harness_test_count", "tests du harness Linux"),
    ):
        count = optional_count(manifest, field)
        if count is not None:
            counts.append(f"{count} {label}")
    images, dimensions, captions = {}, {}, {}
    for key, name in CAPTURES.items():
        capture = manifest.get("captures", {}).get(name)
        if not isinstance(capture, dict):
            raise ValueError(f"The QA manifest does not attest capture {name}")
        expected_hash = capture.get("sha256")
        if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
            raise ValueError(f"Invalid QA hash for capture {name}")
        content = (screenshots / f"{name}.jpg").read_bytes()
        if hashlib.sha256(content).hexdigest() != expected_hash:
            raise ValueError(f"Capture {name} differs from its QA hash")
        actual_dimensions = jpeg_dimensions(content)
        if actual_dimensions != declared_dimensions(capture.get("dimensions")):
            raise ValueError(f"Capture {name} differs from its QA dimensions")
        dimensions[key] = actual_dimensions
        images[key] = "data:image/jpeg;base64," + base64.b64encode(content).decode("ascii")
        tauri = key.startswith("tauri-")
        artifact_name = "tauri" if tauri else "gpui"
        if capture.get("artifact") != artifact_name:
            raise ValueError(f"Capture {name} refers to the wrong QA artifact")
        artifact = manifest.get("capture_artifacts", {}).get(artifact_name)
        if not isinstance(artifact, dict):
            raise ValueError(f"Missing QA artifact for capture {name}")
        binary_hash = artifact.get("sha256")
        if binary_hash is None:
            if not tauri or artifact.get("binary_hash_recorded") is not False:
                raise ValueError(f"Missing QA binary hash attestation for capture {name}")
            binary_label = "hash binaire non attesté"
        else:
            if (not isinstance(binary_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", binary_hash)
                    or artifact.get("binary_hash_recorded") is False):
                raise ValueError(f"Invalid QA binary hash for capture {name}")
            binary_label = f"binaire {binary_hash[:12]}…"
        configuration = artifact.get("configuration")
        if not isinstance(configuration, str) or not configuration.strip():
            raise ValueError(f"Invalid QA binary configuration for capture {name}")
        if tauri:
            captured_on = artifact.get("captured_on")
            if not isinstance(captured_on, str):
                raise ValueError("The Tauri QA artifact has no capture date")
            capture_date = datetime.fromisoformat(captured_on).strftime("%d/%m/%Y")
            provenance = f"Vue/Tauri · capture du {capture_date}"
        else:
            capture_date = datetime.fromisoformat(capture["modified_at"]).strftime("%d/%m/%Y")
            provenance = f"GPUI · capture de production du {capture_date}"
        width, height = actual_dimensions
        captions[key] = (f"{provenance} · {binary_label} "
                         f"({configuration}) · {width} × {height} pixels. "
                         "SHA-256 de la capture et dimensions attestés par le manifeste QA. "
                         "La date et la configuration identifient la provenance de cette "
                         "capture ; aucune QA simultanée ni égalité pixel par pixel "
                         "n’est déduite de dimensions identiques.")
    for name in ("history", "diff", "welcome"):
        if dimensions[name] != dimensions[f"tauri-{name}"]:
            raise ValueError(f"Paired captures for {name} have different pixel dimensions")
    tests = "; ".join(counts) + ". " if counts else "Aucun nombre de tests attesté. "
    verification = (f"Vérifications consignées dans le manifeste QA du {verified_at} : "
                    + tests + f"{len(CAPTURES)} captures affichées vérifiées par SHA-256 "
                    "et dimensions. La provenance des captures est distinguée de celle "
                    "des binaires mesurés dans le contexte ; aucune égalité pixel par pixel "
                    "n’est annoncée.")
    return {"summary": report["summary"], "context": context,
            "launches": report["completed_launches"], "expected_launches": 120,
            "images": images, "capture_captions": captions,
            "qa": manifest, "verification": verification,
            "protocol": {"launches_per_condition": 30, "interactions_per_launch": interactions,
                         "selections_per_application": 60 * interactions,
                         "interactions_per_application": 120 * interactions,
                         "cpu_timebase": f"{timebase['numer']}/{timebase['denom']}"}}


HTML = r'''<!doctype html>
<html lang="fr"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Mangrove — comparaison GPUI / Tauri</title>
<link href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;500;600&family=IBM+Plex+Mono:wght@400;500&display=swap" rel="stylesheet">
<style>
:root{--bg:#f6f3eb;--surface:#fffcf6;--surface2:#ede9df;--border:#d7d5cb;--text:#24342d;--text-dim:#657268;--accent:#32614a;--accent-dim:#e2eade;--tauri:#9d5137;--gpui:#32614a;--warn:#946215;--font-body:'IBM Plex Sans','Avenir Next',Tahoma,sans-serif;--font-mono:'IBM Plex Mono',Menlo,monospace}
@media(prefers-color-scheme:dark){:root{--bg:#17231e;--surface:#1e2d26;--surface2:#29382f;--border:#405045;--text:#e9eee5;--text-dim:#abbcad;--accent:#9ac9aa;--accent-dim:#263c2e;--tauri:#e3a38a;--gpui:#9ac9aa;--warn:#dabb79}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);background-image:radial-gradient(var(--border) .6px,transparent .6px);background-size:24px 24px;color:var(--text);font:16px/1.6 var(--font-body);overflow-wrap:break-word}a{color:var(--accent)}button,select{font:inherit;color:inherit}button{cursor:pointer;background:var(--surface);border:1px solid var(--border);border-radius:5px;padding:7px 12px}button[aria-pressed=true]{background:var(--accent-dim);border-color:var(--accent)}button:focus-visible,a:focus-visible{outline:3px solid var(--accent);outline-offset:3px}.wrap{max-width:1430px;margin:auto;padding:40px;display:grid;grid-template-columns:150px minmax(0,1fr);gap:40px}.toc{position:sticky;top:30px;align-self:start;font:12px/1.6 var(--font-mono)}.toc strong{display:block;margin-bottom:15px;letter-spacing:1px}.toc a{display:block;padding:7px 0;text-decoration:none}.main{min-width:0}header{border-top:3px solid var(--accent);padding-top:23px}.eyebrow{font:12px var(--font-mono);letter-spacing:1px;color:var(--text-dim)}h1{font:normal clamp(40px,6vw,70px)/1.03 Georgia,serif;letter-spacing:-2px;max-width:850px;margin:24px 0}h2{font:normal 32px/1.2 Georgia,serif;margin:0 0 18px}h3{font-size:17px;margin:0 0 6px}p{margin:10px 0 18px}.lead{font-size:20px;max-width:850px}.muted,small{color:var(--text-dim)}section{padding:42px 0;border-bottom:1px solid var(--border);scroll-margin-top:70px}.controls{display:flex;gap:8px;flex-wrap:wrap;margin:18px 0}.hero{background:var(--accent-dim);border:1px solid var(--border);border-left:4px solid var(--accent);padding:24px;margin:22px 0}.kpis{display:grid;grid-template-columns:1.4fr 1fr 1fr;gap:16px}.kpis>*{min-width:0;background:var(--surface);padding:22px;border:1px solid var(--border)}.value{font:40px/1.2 Georgia,serif;margin:8px 0;color:var(--accent)}.label{font:12px/1.5 var(--font-mono);color:var(--text-dim)}.table-wrap{overflow:auto;border:1px solid var(--border);background:var(--surface)}table{border-collapse:collapse;width:100%;min-width:700px;font-size:14px}thead{position:sticky;top:0}th{background:var(--surface2);font:12px var(--font-mono);text-align:left;padding:14px}td{padding:13px 14px;border-top:1px solid var(--border);vertical-align:top}tr:nth-child(even){background:color-mix(in srgb,var(--surface2) 40%,var(--surface))}tr:hover{background:var(--accent-dim)}td.num{font-family:var(--font-mono);text-align:right;font-variant-numeric:tabular-nums;white-space:nowrap}small{display:block;font-size:12px;line-height:1.5}.tauri{color:var(--tauri)}.gpui{color:var(--gpui)}.bars{display:grid;grid-template-columns:1fr 1fr;gap:20px;margin:20px 0}.bars>*{min-width:0;background:var(--surface);padding:20px;border:1px solid var(--border)}.barline{margin-top:13px}.barline span{display:flex;justify-content:space-between;font:12px var(--font-mono);gap:10px}.bartrack{height:9px;background:var(--surface2);margin-top:7px}.bartrack i{display:block;height:100%;background:var(--gpui)}.barline:first-of-type .bartrack i{background:var(--tauri)}figure{margin:20px 0}figure img{display:block;width:100%;height:auto;border:1px solid var(--border);border-radius:7px;background:#202127}figcaption{font-size:13px;margin-top:9px;color:var(--text-dim)}.note{border-left:3px solid var(--warn);padding:8px 18px;background:var(--surface);margin:20px 0}.features{display:grid;grid-template-columns:1fr 1fr;gap:12px}.features>*{min-width:0;border-top:1px solid var(--border);padding:14px 0}.features strong{font-size:15px}.features p{font-size:14px;color:var(--text-dim);margin:4px 0}details{border:1px solid var(--border);padding:16px 20px;background:var(--surface);margin:16px 0}summary{cursor:pointer;font-weight:600}pre{white-space:pre-wrap;font:12px/1.6 var(--font-mono);overflow-wrap:anywhere;color:var(--text-dim)}code{font-family:var(--font-mono);font-size:.85em}footer{padding-top:24px;font-size:13px;color:var(--text-dim)}
@media(max-width:1000px){.wrap{display:block;padding:20px}.toc{top:0;z-index:20;display:flex;overflow:auto;gap:20px;background:var(--bg);margin:-20px -20px 20px;padding:12px 20px;border-bottom:1px solid var(--border)}.toc strong{display:none}.toc a{white-space:nowrap;padding:0}h1{letter-spacing:-1px}.kpis{grid-template-columns:1fr}.bars,.features{grid-template-columns:1fr}section{padding:30px 0}}
@media print{.toc,.controls{display:none}.wrap{display:block;padding:0}section{break-inside:avoid}body{background:#fff;color:#000}}
</style></head><body><div class="wrap"><nav class="toc" aria-label="Sections"><strong>MANGROVE / GPUI</strong><a href="#mesures">01 · Résultats</a><a href="#rendu">02 · Cadence</a><a href="#fidelite">03 · Fidélité</a><a href="#methode">04 · Méthode</a></nav><main class="main">
<header><p class="eyebrow">COMPARAISON RELEASE · MACOS ARM · <span id="launch-count"></span> LANCEMENTS</p><h1>Deux interfaces.<br>Le même moteur Git.</h1><p class="lead">Le port GPUI reprend l’interface et ses fonctionnalités pour comparer le coût réel de l’application avec Vue / Tauri.</p><p class="muted" id="machine"></p><p class="muted" id="series-stage"></p></header>
<section id="mesures"><h2>01 · Coût et réactivité</h2><p>Choisissez la condition de cache. Les valeurs p50 et p95 décrivent les <strong id="independent-launches"></strong> de chaque application. Pour les interactions répétées, ce sont les quantiles des médianes par lancement.</p><div class="controls" id="cache-controls"><button data-cache="cold_app_cache" aria-pressed="true">Cache applicatif neuf</button><button data-cache="warm_app_cache" aria-pressed="false">Cache réutilisé</button></div><div class="hero" id="verdict"></div><div class="kpis" id="kpis"></div><div class="bars" id="bars"></div><div class="table-wrap"><table><thead><tr><th>Mesure</th><th class="tauri">Tauri · p50 / p95</th><th class="gpui">GPUI · p50 / p95</th><th>Écart des p50</th></tr></thead><tbody id="metrics"></tbody></table></div><div class="note"><strong>Les délais d’affichage sont des proxys.</strong> L’état prêt est observé à la cadence de rendu propre à chaque interface. Tauri attend ensuite deux callbacks rAF ; GPUI une soumission de frame ultérieure. Ces différences empêchent d’interpréter leurs ratios comme un gain physique clic → pixels. Les ressources mémoire et CPU ont une mesure commune.</div></section>
<section id="rendu"><h2>02 · Cadence du défilement</h2><p>Trois secondes à 2 400 pixels/s dans l’historique et dans un patch de 2 000 lignes. Les p50 et p95 ci-dessous portent sur les <strong>intervalles regroupés de callbacks rAF / soumissions GPUI</strong>, sans mesure du scanout de l’écran.</p><div class="table-wrap"><table><thead><tr><th>Parcours</th><th class="tauri">Tauri · p50 / p95</th><th class="gpui">GPUI · p50 / p95</th></tr></thead><tbody id="frames"></tbody></table></div><p class="muted">Une cadence de soumission supérieure ne prouve pas, à elle seule, un framerate visible supérieur. Le rendu du WebView et les soumissions natives utilisent des mécanismes différents.</p></section>
<section id="fidelite"><h2>03 · Fidélité de l’interface</h2><p>Disposition, couleurs des trois thèmes, icônes, graphe, densité des lignes et tailles de texte repris de l’application existante. La date, la configuration et l’attestation du binaire de chaque capture figurent dans sa légende et dans le manifeste QA. Les vues correspondantes ont les mêmes dimensions ; elles ne certifient ni une QA simultanée ni une égalité pixel par pixel. Cette comparaison est qualitative ; les autres captures couvrent les thèmes et le blame du port GPUI.</p><div class="controls" id="image-controls"><button data-image="history" aria-pressed="true">Historique · GPUI</button><button data-image="tauri-history" aria-pressed="false">Historique · Tauri</button><button data-image="diff" aria-pressed="false">Diff · GPUI</button><button data-image="tauri-diff" aria-pressed="false">Diff · Tauri</button><button data-image="welcome" aria-pressed="false">Accueil · GPUI</button><button data-image="tauri-welcome" aria-pressed="false">Accueil · Tauri</button><button data-image="mangrove" aria-pressed="false">Mangrove</button><button data-image="macchiato" aria-pressed="false">Macchiato</button><button data-image="blame" aria-pressed="false">Blame</button></div><figure><img id="capture" alt="Capture de la version native GPUI"><figcaption id="capture-caption"></figcaption></figure><div class="features"><article><strong>Navigation et historique</strong><p>Onglets, accueil, récents, recherche, arbre de références, Solo, historique et graphe virtualisés.</p></article><article><strong>Contexte Git</strong><p>Changements indexés/non indexés/non suivis, worktrees liés, stashes et leurs trois zones, fetch et auto-fetch.</p></article><article><strong>Consultation des fichiers</strong><p>Diff central/compact, coloration, contenu, historique avec renommages, blame et navigation vers les commits.</p></article><article><strong>Interactions natives</strong><p>Paramètres, tailles de texte, panneaux redimensionnables, sélection/copie, raccourcis, focus et menus natifs.</p></article></div><div class="note"><strong>Écarts encore identifiés.</strong> La rasterisation native du texte et certains retours à la ligne peuvent encore différer. Les interlignes, la gouttière du message, les sélections de branches, les compteurs et l’accueil ont été ajustés après la revue appariée. Tree-sitter et Shiki ne découpent pas tous les tokens de la même manière ; Kotlin et CMake ont un fallback lexical. Les contrôles et le raster de texte natifs peuvent différer. QA réalisée sur macOS ARM ; Windows/Linux, VoiceOver complet, signature et notarisation restent à valider.</div><p id="qa-verification"></p></section>
<section id="methode"><h2>04 · Périmètre de la mesure</h2><div class="features"><article><strong>Mêmes données</strong><p>10 007 commits, 46 fichiers changés, deux petits diffs TypeScript colorés et le même patch tronqué à 2 000 lignes. Le grand patch revient au texte simple dans les deux interfaces.</p></article><article><strong>Répétitions</strong><p><span id="repetitions"></span></p></article><article><strong>Mémoire entière</strong><p>Tauri et ses processus XPC WebContent/GPU/Networking attribués par PID responsable. RSS et empreinte physique distinctes, échantillonnées toutes les 100 ms.</p></article><article><strong>Conditions</strong><p>Builds release, thème Classique, contenu 1 440 × 888 px, préférences isolées et auto-fetch désactivé. Cache OS chaud ou incontrôlé. Lancement via LaunchServices et activation effective attestée sur l’écran principal HiDPI à 120 Hz.</p></article></div><details open><summary>Limites d’interprétation</summary><ul><li>Ce comparatif porte sur deux implémentations complètes. Le diff Vue crée toutes les lignes DOM du patch ; le port GPUI virtualise les lignes de code. Les résultats mêlent donc choix d’implémentation et framework, et ne prouvent pas un gain du seul moteur de rendu GPUI.</li><li>Le moteur Git/SQLite est partagé. La mesure ne démontre pas une accélération des calculs Git.</li><li>Les interactions d’un même processus sont corrélées ; les quantiles regroupés sont conservés dans les données brutes.</li><li>La phase de sélection des diffs inclut la recherche DOM du fichier et des attentes supplémentaires côté Tauri. Ce n’est pas un benchmark pur des parseurs de coloration.</li><li>Le délai d’ouverture du gros diff GPUI comprend un callback de planification supplémentaire. La distance réellement parcourue pendant les scrolls n’est pas enregistrée ; seule la même commande est vérifiée.</li><li>Les ressources sont observées dès que LaunchServices retourne le PID, y compris pendant l’attente de son activation. La création initiale du processus peut précéder ces observations. Le pic observé peut manquer un pic entre deux échantillons. La somme des RSS peut compter plusieurs fois des pages partagées.</li><li>Les compteurs CPU Mach sont convertis en nanosecondes avec la timebase locale <span id="cpu-timebase"></span>. La conversion est appliquée pendant la collecte, vérifiée par une calibration indépendante et conservée dans le contexte du rapport ; <a href="https://github.com/apple-oss-distributions/xnu/blob/main/tests/recount/recount_perf_tests.c#L183-L215">les tests officiels Apple appliquent cette conversion</a>.</li><li>WindowServer, les pilotes et l’énergie globale ne sont pas attribués. Aucun gain de batterie n’est déduit.</li><li>Les applications habituelles de l’utilisateur sont restées ouvertes : la charge de fond n’est pas neutralisée. Les rectangles WindowServer relevés à l’activation peuvent encore refléter l’animation d’ouverture ; le viewport utile est contrôlé dans chaque application.</li><li>Résultats sur une machine et une fixture ; confirmer sur les dépôts et plateformes de production avant migration.</li></ul></details><p id="post-series"></p><p id="distribution"></p><details><summary>Contexte et identifiants des builds mesurés</summary><pre id="context"></pre></details><p>Les données brutes et le protocole reproductible sont disponibles dans <code>docs/performance-gpui.json</code> et <code>docs/performance-gpui-protocol.md</code>. Le port est isolé dans <code>src-gpui/</code>.</p></section><footer>Issue #9 · Mangrove · Rapport local, captures et données intégrés dans ce fichier.</footer>
</main></div><script>
const DATA=__DATA__;
let mode='cold_app_cache';
const fmt=(v)=>v==null?'—':new Intl.NumberFormat('fr-FR',{maximumFractionDigits:1}).format(v);
const metric=(impl,path)=>path.reduce((v,k)=>v?.[k],DATA.summary[mode][impl]);
const rows=[
['RSS au repos',['resources','idle','resident_mib'],'MiB','Médiane de la phase inactive, après 500 ms de stabilisation.'],
['Empreinte physique au repos',['resources','idle','physical_footprint_mib'],'MiB','Mesure macOS distincte du RSS.'],
['Pic de RSS',['resources','peak_resident_mib'],'MiB','Somme simultanée de tous les processus attribués.'],
['Pic d’empreinte physique',['resources','peak_physical_footprint_mib'],'MiB','Maximum observé toutes les 100 ms, à partir du PID retourné par LaunchServices.'],
['CPU au repos',['resources','idle','cpu_percent_one_core'],'%','100 % correspond à un cœur occupé.'],
['CPU · défilement historique',['resources','history_scroll','cpu_percent_one_core'],'%','Même vitesse et durée dans les deux applications.'],
['CPU · défilement diff',['resources','diff_scroll','cpu_percent_one_core'],'%','Patch de 2 000 lignes, sans coloration dans les deux applications.'],
['Démarrage · état exploitable observé',['timings','startup_history_cpu_ready','per_launch_p50_ms'],'ms','Depuis le lancement, état prêt observé à la cadence des frames.'],
['Démarrage · occasion de rendu',['timings','startup_history','per_launch_p50_ms'],'ms','Proxys de présentation différents ; voir la réserve ci-dessous.'],
['Sélection du commit · état prêt',['timings','select_details_cpu_ready','per_launch_p50_ms'],'ms','Médiane des sélections par lancement, première lecture du diff comprise.'],
['Sélection du diff coloré · état prêt',['timings','select_colored_diff_cpu_ready','per_launch_p50_ms'],'ms','Médiane des sélections par lancement, vrais handlers et caches.'],
['Ouverture du gros diff · état prêt',['timings','open_heavy_diff_cpu_ready','per_launch_p50_ms'],'ms','Première ouverture du même fichier dans chaque fenêtre.']
];
function pair(impl,path,unit){const m=metric(impl,path);return `${fmt(m?.p50)} / ${fmt(m?.p95)} ${unit}`}
function delta(a,b){if(!a||b==null)return '—';let n=(b/a-1)*100;return (n>0?'+':'')+fmt(n)+' %'}
function render(){document.querySelectorAll('[data-cache]').forEach(b=>b.setAttribute('aria-pressed',b.dataset.cache===mode));
document.getElementById('metrics').innerHTML=rows.map(([name,path,unit,note])=>`<tr><td>${name}<small>${note}</small></td><td class="num tauri">${pair('tauri',path,unit)}</td><td class="num gpui">${pair('gpui',path,unit)}</td><td class="num">${delta(metric('tauri',path)?.p50,metric('gpui',path)?.p50)}</td></tr>`).join('');
const t=metric('tauri',['resources','idle','physical_footprint_mib'])?.p50,g=metric('gpui',['resources','idle','physical_footprint_mib'])?.p50;
const ht=metric('tauri',['resources','history_scroll','cpu_percent_one_core'])?.p50,hg=metric('gpui',['resources','history_scroll','cpu_percent_one_core'])?.p50,dt=metric('tauri',['resources','diff_scroll','cpu_percent_one_core'])?.p50,dg=metric('gpui',['resources','diff_scroll','cpu_percent_one_core'])?.p50;document.getElementById('verdict').innerHTML=`<strong>Empreinte physique au repos : ${delta(t,g)} avec GPUI.</strong><p>CPU de défilement : ${delta(ht,hg)} dans l’historique et ${delta(dt,dg)} dans le diff. Chaque application conserve sa cadence par défaut ; le scénario observe environ ${fmt(1000/metric('gpui',['frames','history_scroll','interval_ms','pooled_ms'])?.p50)} soumissions/s GPUI et ${fmt(1000/metric('tauri',['frames','history_scroll','interval_ms','pooled_ms'])?.p50)} callbacks rAF/s Tauri. Ces données justifient une lecture distincte de la mémoire, du coût CPU et de la cadence.</p>`;document.getElementById('kpis').innerHTML=`<article><div class="label">EMPREINTE PHYSIQUE AU REPOS · GPUI</div><div class="value">${fmt(g)} MiB</div><small>Tauri : ${fmt(t)} MiB · écart ${delta(t,g)}</small></article><article><div class="label">LANCEMENTS TERMINÉS</div><div class="value">${fmt(DATA.launches)} / ${fmt(DATA.expected_launches)}</div><small>${fmt(DATA.protocol.launches_per_condition)} par application et condition</small></article><article><div class="label">INTERACTIONS PAR APPLICATION</div><div class="value">${fmt(DATA.protocol.interactions_per_application)}</div><small>${fmt(DATA.protocol.selections_per_application)} commits + ${fmt(DATA.protocol.selections_per_application)} diffs colorés</small></article>`;
document.getElementById('bars').innerHTML=[rows[1],rows[7]].map(([name,path,unit,note])=>{let vals=['tauri','gpui'].map(i=>metric(i,path)?.p50||0),max=Math.max(...vals,1);return `<article><h3>${name}</h3><small>${note}</small>${['tauri','gpui'].map((i,ix)=>`<div class="barline"><span><b>${i==='tauri'?'Tauri + WebView':'GPUI'}</b><b>${fmt(vals[ix])} ${unit}</b></span><div class="bartrack"><i style="width:${Math.max(1,vals[ix]/max*100)}%"></i></div></div>`).join('')}</article>`}).join('');
document.getElementById('frames').innerHTML=['history_scroll','diff_scroll'].map(p=>{let path=['frames',p,'interval_ms','pooled_ms'];return `<tr><td>${p==='history_scroll'?'Historique':'Diff 2 000 lignes'}<small>Intervalles regroupés, corrélés au sein de chaque lancement.</small></td><td class="num tauri">${pair('tauri',path,'ms')}</td><td class="num gpui">${pair('gpui',path,'ms')}</td></tr>`}).join('');}
document.querySelectorAll('[data-cache]').forEach(b=>b.addEventListener('click',()=>{mode=b.dataset.cache;render()}));
function showImage(key){document.querySelectorAll('[data-image]').forEach(b=>b.setAttribute('aria-pressed',b.dataset.image===key));const img=document.getElementById('capture');img.src=DATA.images[key];img.alt=`Mangrove ${key.startsWith('tauri-')?'Vue/Tauri':'GPUI'} : ${key.replace('tauri-','')}`;document.getElementById('capture-caption').textContent=DATA.capture_captions[key];}
document.querySelectorAll('[data-image]').forEach(b=>b.addEventListener('click',()=>showImage(b.dataset.image)));
showImage('history');document.getElementById('context').textContent=JSON.stringify({mesures:DATA.context,qa:DATA.qa},null,2);const binaries=DATA.context.artifacts?.binaries||DATA.context.artifacts;if(binaries){document.getElementById('distribution').innerHTML=`<strong>Coût de distribution :</strong> exécutable GPUI ${fmt(binaries.gpui.bytes/1048576)} MiB, Tauri ${fmt(binaries.tauri.bytes/1048576)} MiB. Ces tailles excluent les bibliothèques système ; le moteur WebView de Tauri est fourni par macOS.`;}document.getElementById('machine').textContent=`${DATA.context.hardware.cpu} · ${fmt(DATA.context.hardware.ram_bytes/1073741824)} GiB de RAM · ${DATA.context.os} · zone de contenu 1 440 × 888 px`;
document.getElementById('launch-count').textContent=fmt(DATA.launches);
document.getElementById('independent-launches').textContent=`${fmt(DATA.protocol.launches_per_condition)} lancements indépendants`;
document.getElementById('repetitions').textContent=`${fmt(DATA.protocol.launches_per_condition)} lancements × 2 applications × 2 conditions. ${fmt(DATA.protocol.interactions_per_launch)} sélections de commits et ${fmt(DATA.protocol.interactions_per_launch)} sélections de diffs colorés par fenêtre ; ordre des applications alterné.`;
document.getElementById('cpu-timebase').textContent=DATA.protocol.cpu_timebase;
document.getElementById('qa-verification').textContent=DATA.verification;
document.getElementById('post-series').textContent=DATA.context.post_series_validation?.note||'';
document.getElementById('series-stage').textContent=DATA.context.series_stage?`Étape mesurée : ${DATA.context.series_stage}. Les hashes ci-dessous identifient les artefacts effectivement comparés.`:'';
render();
</script></body></html>'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, default=ROOT / "docs/performance-gpui.json")
    parser.add_argument("--output", type=Path, default=ROOT / "docs/performance-gpui.html")
    parser.add_argument("--screenshots", type=Path, default=ROOT / "src-gpui/screenshots")
    parser.add_argument("--qa-manifest", type=Path,
                        default=ROOT / "src-gpui/screenshots/qa-manifest.json")
    args = parser.parse_args()
    try:
        report = json.loads(args.input.read_text(encoding="utf-8"))
        manifest = json.loads(args.qa_manifest.read_text(encoding="utf-8"))
        data = report_data(report, manifest, args.screenshots)
        serialized = json.dumps(data, ensure_ascii=False, allow_nan=False).replace("</", "<\\/")
        html = HTML.replace("__DATA__", serialized)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(html, encoding="utf-8")
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.error(str(error))
    print(args.output.resolve())


if __name__ == "__main__":
    main()
