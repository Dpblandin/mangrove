# QA native des worktrees, stashes et Solo

Validation du 30 septembre 2026 sur macOS ARM64, dans le bundle Tauri release avec un identifiant de QA distinct. Le processus est fermé puis relancé après les reconstructions. La fixture synthétique contient **10 007 commits** dans son historique ordinaire, un merge à trois parents et un commit avec 46 changements mixtes. Sa copie de QA ajoute des branches, trois worktrees liés et un stash ; aucun dépôt réel n’est modifié pour ces scénarios.

## Parcours vérifiés

- **Solo local** : `qa/solo` réduit le parcours de 10 007 à **9 953 commits**. Les nœuds et connexions du graphe sont visibles ; la branche du worktree reste explicitement **HEAD · main**.
- **Union locale/distante** : le menu contextuel de `origin/feat/commit-search` propose « Ajouter au Solo ». Entrée ajoute la branche et porte le parcours à **9 998 commits** ; le bandeau nomme les deux branches.
- **Recherche et sortie** : `showcase`, présent seulement sur `main`, renvoie zéro résultat dans cette union Solo. « Quitter le mode Solo » conserve la recherche et retrouve le commit attendu, puis effacer la recherche restaure les 10 007 commits.
- **Clavier** : depuis le filtre des références, Tab puis Tab place le focus sur `main`. Maj + F10 ouvre son menu ; Entrée active Solo. Le bandeau, la sélection et les voies du merge sont visibles. Les boutons Solo restent accessibles séparément du bouton de navigation.
- **Worktrees** : l’ouverture de la section affiche le principal, un worktree lié verrouillé avec son motif, un worktree en HEAD détachée et une inscription indisponible désactivée. Le message d’indisponibilité reste court et lisible. Ouvrir le worktree lié crée un deuxième onglet avec **HEAD · qa/linked** ; cliquer le principal réactive le premier onglet.
- **Stash et comparaisons** : la liste affiche message, auteur, date et commit de base. Le panneau annonce « Index sauvegardé → répertoire de travail sauvegardé », « Commit de base → index sauvegardé » et « Arbre vide → fichiers non suivis sauvegardés ». Ouvrir `src/history.ts` dans les deux premières zones affiche respectivement la modification non indexée et la modification indexée ; `untracked-qa.ts` affiche sa ligne ajoutée dans la troisième. Changer de zone ferme le diff précédent.
- **Diff central/compact et actualisation** : le patch TypeScript est coloré dans les deux emplacements. Une actualisation sans changement conserve le stash et son diff. Une suppression externe du stash synthétique, suivie d’une actualisation, vide la liste et retire les anciens détails et le patch ; le panneau signale l’indisponibilité.
- **États vides** : ouvrir la fixture d’origine affiche seulement le worktree principal et « Aucun stash dans ce dépôt ».
- **Déplacement externe de branche** : déplacer la référence synthétique `qa/solo` dix commits en arrière, puis actualiser, conserve le Solo et **HEAD · main**, avec **9 943 commits** et le nouveau dernier commit.
- **Navigation normale hors Solo** : après isolation de `qa/solo`, cliquer `main` restaure les **10 007 commits** et sélectionne son dernier commit, sans faux message « référence non indexée ». Le scénario est rejoué après reconstruction et relance du correctif. L’attente réutilise la progression d’indexation existante ; les tests couvrent aussi son annulation et son expiration.

Les suppressions et déplacements de ce scénario sont effectués par Git uniquement dans la copie synthétique, extérieurement à l’application. Mangrove ne propose aucune de ces opérations.

## Dépôt source et contrôles

Un instantané local de **186 entrées**, dans le dépôt synthétique et ses worktrees disponibles, compare le contenu des fichiers, les cibles de symlinks, les types d’entrées et les dates de modification en nanosecondes, dossiers et `.git` compris. Après les consultations initiales et le benchmark : **aucune différence**. Un nouvel instantané pris après les modifications externes intentionnelles compare les **185 entrées** restantes après la navigation et l’actualisation : **aucune différence**. Les instantanés restent locaux.

Les tests Rust vérifient aussi les snapshots, l’union des racines Solo, tous les parents de merge, les ancêtres communs, la recherche et la pagination, les références déplacées ou supprimées, les inscriptions manquantes et les motifs de verrou non UTF-8, les dépôts bare et unborn, les stashes à deux ou trois parents, les fichiers binaires et les limites de diff. Les tests Vue contrôlent le chargement à la demande, les réponses tardives, les annulations et la libération des sélections. Les revues déléguées `refactor` frontend/backend puis `simplify` ont été appliquées.

Contrôles exécutés : **302 tests Vue**, **61 tests Rust** (2 unitaires et 59 d’intégration), **8 tests de fixture**, format/lint/types, Clippy workspace avec avertissements interdits et compilation du bundle macOS release. Tous passent.

Les mesures du moteur sur la fixture et sur un dépôt réel de plus de 12 000 commits sont publiées dans [performance.md](performance.md) et [performance-context.json](performance-context.json), sous forme agrégée.

## Portée

La QA interactive porte sur macOS. La CI vérifie le moteur et les commandes natives sur macOS, Linux et Windows ; cela ne remplace pas une QA interactive des deux autres plateformes. Les mesures de performance excluent le rendu, la coloration et l’IPC. Les limites et règles d’actualisation sont décrites dans [repository-context.md](repository-context.md).

## Liste compacte des worktrees

La retouche de présentation est vérifiée dans le navigateur collaboratif avec le composant Vue réel, les styles de l'application et six worktrees synthétiques. Chaque entrée occupe une ligne de **28 px**, nommée par sa branche ; le principal porte une maison, le worktree ouvert reste seul surligné en vert et coché, et la HEAD détachée conserve son SHA court. Les chemins ne figurent plus dans le texte visible de la liste. L'attribut de l'infobulle native contient le nom complet, le répertoire de travail et les éventuels motifs de verrouillage ou d'indisponibilité, également présents dans le libellé accessible.

À **320 px** puis **190 px** de largeur, les noms longs restent tronqués avec une ellipse, les icônes d'état restent visibles et les lignes ne débordent pas. Le clic sur le worktree verrouillé émet toujours son chemin ; Tab puis Entrée ouvrent l'entrée suivante, et Tab ignore l'inscription indisponible. Replier la section retire ses lignes. Les **302 tests Vue** et les contrôles de format, lint et types passent. Cette vérification du composant complète les parcours natifs précédents ; elle ne rejoue pas la lecture Git ni l'ouverture des onglets natifs.
