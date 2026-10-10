# Changelog

Évolutions notables de Mangrove, regroupées par version. Chaque section est générée depuis les commits par `scripts/release.sh`, relue avant le tag, puis reprise telle quelle dans la release GitHub.

## [0.1.0](https://github.com/Dpblandin/mangrove/releases/tag/v0.1.0) - 2026-10-10

Première version publique de Mangrove, client Git desktop natif en lecture seule.

### Nouveautés

- Dépôts locaux, bare et worktrees, avec récents, favoris par profil et onglets qui retrouvent leur sélection, leur recherche et leur position de lecture.
- Historique avec graphe des branches et des merges, colonne des références et recherche par message, auteur, SHA ou référence.
- Branches locales et distantes, tags, filtre et mode Solo ; consultation des worktrees et des stashes.
- Détails des commits, diffs avec coloration syntaxique et aperçus d’images avant/après.
- Contenu d’un fichier à une révision, historique qui suit les renommages et blame.
- Modifications locales indexées, non indexées et non suivies, en lecture seule.
- Fetch manuel ou automatique, avec des erreurs résumées et le détail de Git.
- Profils et comptes GitLab, GitHub et Bitbucket (dépôts, PR/MR, CI), clonage HTTPS ou SSH ; jetons dans le coffre de l’OS.
- Cinq thèmes, palette de commandes, navigation au clavier, panneaux redimensionnables et zoom.

### Distribution

- Builds macOS Apple Silicon et Intel, Linux x86_64 et Windows x86_64. Les builds macOS sont signés ad hoc, sans notarisation, et les builds Windows ne sont pas signés.
- Licence FSL-1.1-MIT.
