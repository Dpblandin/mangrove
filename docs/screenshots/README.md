# Captures du README

Captures réelles de Mangrove **0.3.0**, prises le 11 octobre 2026 sur macOS avec le thème **Classique**, dans une fenêtre de 1440 × 888 points (JPEG Retina de 2880 × 1840 pixels, barre de titre incluse).

- `main-window.jpg` : historique, branches et détails du commit de démonstration comportant 46 fichiers modifiés.
- `diff.jpg` : diff TypeScript côte à côte de `packages/core/src/cache/HistoryCache.ts`, avec coloration syntaxique et surlignage des mots modifiés.

Le dépôt fictif est généré avec `node scripts/create-fixture.mjs --commits 32 --large-change`. Deux commits d’exemple ajoutent un état du cache puis modifient ses paramètres et sa limite pour illustrer le diff. Les auteurs sont fictifs et leurs adresses utilisent `example.test`.

La session de capture utilise des fichiers de préférences et de profils isolés via `MANGROVE_GPUI_PREFERENCES` et `MANGROVE_PROFILES_PATH`, sans compte connecté et avec l’auto-fetch désactivé. Le dépôt de démonstration et les fichiers de session restent dans `.mangrove-dev/`, ignoré par Git.
