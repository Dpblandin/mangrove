# Changelog

Évolutions notables de Mangrove, regroupées par version. Chaque section est générée depuis les commits par `scripts/release.sh`, relue avant le tag, puis reprise telle quelle dans la release GitHub.

## [0.4.1](https://github.com/Dpblandin/mangrove/releases/tag/v0.4.1) - 2026-10-12

### Corrections

- *(syntax)* Color diff lines within their whole files
- *(syntax)* Read PHP hunks below the opening tag as code

## [0.4.0](https://github.com/Dpblandin/mangrove/releases/tag/v0.4.0) - 2026-10-11

### Nouveautés

- *(update)* Update the application automatically from signed releases, with its settings in Settings → Mises à jour

## [0.3.0](https://github.com/Dpblandin/mangrove/releases/tag/v0.3.0) - 2026-10-11

### Nouveautés

- Thème clair 3615 Profs, inspiré des écrans texte gris des logiciels de gestion de l’Éducation nationale : texte noir, curseur bleu et interface en Courier New.

## [0.2.1](https://github.com/Dpblandin/mangrove/releases/tag/v0.2.1) - 2026-10-11

### Corrections

- L’application ne peut plus se figer quand l’indexation, la lecture de l’historique ou un fetch ouvrent et ferment des connexions au cache au même moment. SQLite passe de 3.51.1 à 3.53.2, qui corrige cet interblocage ainsi qu’un bug rare de corruption des bases en mode WAL.

## [0.2.0](https://github.com/Dpblandin/mangrove/releases/tag/v0.2.0) - 2026-10-11

### Nouveautés

- Avance et retard de chaque branche locale sur son upstream dans la sidebar (`↑3 ↓12`), et « disparu » quand l’upstream n’existe plus ; la puce HEAD de l’historique affiche les mêmes valeurs. [#10](https://github.com/Dpblandin/mangrove/issues/10)
- Mots modifiés surlignés à l’intérieur des lignes changées d’un diff. [#11](https://github.com/Dpblandin/mangrove/issues/11)
- Diffs côte à côte dans la vue centrale, au choix pour chaque profil, avec sélection et copie d’un seul côté. [#11](https://github.com/Dpblandin/mangrove/issues/11)
- Fichiers volumineux lus jusqu’à une limite réglable dans les paramètres, de 2 à 64 Mio (8 par défaut), au lieu d’être ignorés au-delà de 2 Mio ; un fichier plus gros s’affiche sur demande.
- Thème Octopus.
- Paramètres retravaillés : erreurs sous chaque champ, Entrée pour valider, profil actif dans l’en-tête, clés SSH vérifiées avant usage et déconnexion confirmée. [#2](https://github.com/Dpblandin/mangrove/issues/2)

### Corrections

- Les boutons d’en-tête des diffs, de l’inspecteur et de l’explorateur de fichiers n’agissent plus deux fois par appui sur Entrée ou Espace. [#11](https://github.com/Dpblandin/mangrove/issues/11)

### Mise à jour

- Les dépôts déjà indexés sont réindexés une fois au premier lancement de cette version.

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
