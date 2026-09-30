# Worktrees, stashes et mode Solo

Mangrove consulte ces données par des commandes natives Rust/git2. La consultation ne change ni HEAD, ni les fichiers, ni l'index, ni les références, ni la configuration des dépôts. Aucun checkout, création/suppression de worktree ou application/suppression de stash n'est exposé.

## Worktrees

La section Worktrees se charge à son ouverture et présente une ligne compacte par branche, ou HEAD détachée avec son SHA court. Le worktree principal porte une icône de maison ; le worktree ouvert est surligné en vert et coché. Le survol affiche le chemin complet du répertoire de travail, le type de worktree et les éventuels motifs de verrouillage ou d'indisponibilité. Ces informations sont aussi incluses dans le libellé accessible. Les entrées verrouillées ou indisponibles conservent leurs icônes d'état. Un verrou Git interdit certaines opérations de maintenance ; il n'empêche pas la consultation d'un worktree disponible. Les inscriptions dont le dossier manque restent affichées sans réparation ou nettoyage.

Ouvrir une entrée disponible utilise son dossier de travail dans le parcours d'onglets existant. Une entrée déjà ouverte active cet onglet. Chaque worktree conserve son identité de session et son index Git local ; les gardes existantes interdisent un cache applicatif dans le worktree, ses dossiers Git ou le worktree principal. Un dépôt bare indique explicitement qu'il ne possède pas de répertoire de travail.

La réponse contient au plus 256 entrées, principal compris, et annonce sa troncature. Les lectures de noms et inscriptions sont natives ; aucun processus Git ou shell n'est exécuté par l'application.

## Stashes

La section Stashes se charge à son ouverture. Elle présente au plus les 128 entrées les plus récentes du reflog de `refs/stash`, avec leur message, date, auteur et commit de base. La troncature est annoncée. Un stash est identifié par son SHA, indépendamment de son numéro `stash@{n}`, qui peut changer après une opération externe.

La sélection charge ses listes de fichiers ; le patch et sa coloration se chargent lorsque l'utilisateur ouvre un fichier. Les trois comparaisons ont des onglets distincts :

- **Répertoire de travail** : arbre du deuxième parent (index sauvegardé) → arbre du commit de stash. Affiche les changements non indexés sauvegardés.
- **Index** : arbre du premier parent (commit de base) → arbre du deuxième parent. Affiche les changements indexés sauvegardés.
- **Non suivis**, seulement si le stash possède un troisième parent : arbre vide → arbre de ce troisième parent.

Le panneau indique le sens de la comparaison et conserve ses identifiants de révision. Un fichier modifié avant et après indexation peut apparaître dans les deux premières zones, avec deux diffs différents. Les renommages sont détectés ; les clés opaques des chemins conservent les noms Git non UTF-8. Le backend vérifie que le SHA demandé appartient encore à la liste consultable du reflog avant de lire détails ou diff : un stash supprimé extérieurement ne reste pas accessible par une ancienne sélection.

Les listes de fichiers, patches, blobs et couleurs utilisent les mêmes limites que les autres diffs : 5 000 fichiers par zone, fichiers de 2 Mio, 2 000 lignes ou 512 Kio par patch, cache et coloration bornés. Les limites de réponses ne bornent pas toutes les allocations internes de libgit2 : la lecture du reflog, la comparaison des arbres et la détection des renommages restent à mesurer sur des cas extrêmes.

Les commits techniques de stash restent exclus des racines de l'historique général et de la recherche. Consulter les stashes n'indexe pas leurs instantanés dans le graphe normal.

## Solo

Le bouton Solo d'une branche locale ou distante, ou son menu contextuel accessible au clavier, ajoute cette référence au filtre. Une branche déjà sélectionnée est retirée ; retirer la dernière restaure l'historique complet. Plusieurs branches forment l'union de leurs historiques. Le bandeau indique les branches sélectionnées et permet de quitter le mode.

Le moteur parcourt tous les commits accessibles depuis les références sélectionnées, avec tous les parents des merges et les ancêtres communs dédupliqués. Il recalcule les positions et les voies du graphe pour ce parcours. HEAD n'est pas ajouté comme racine lorsque le Solo est actif ; le nom de la branche réellement active reste visible dans l'en-tête. Les modifications locales restent celles du worktree ouvert.

La pagination, la recherche et la position des commits utilisent le même parcours. Une recherche textuelle conserve le comportement existant : les liens entre résultats non adjacents sont masqués. Les métadonnées et l'index FTS5 sont réutilisés ; changer le Solo renouvelle seulement le parcours et le graphe dans le cache applicatif du worktree. Les réponses des anciennes générations ne peuvent pas remplacer la nouvelle vue.

Le clic normal sur une branche conserve la navigation vers son dernier commit. Si ce commit est hors du parcours Solo, le filtre est quitté pour permettre cette navigation. Le Solo est conservé lors d'une actualisation du dépôt actif, puis recalculé depuis les références actuelles ; les branches supprimées extérieurement quittent le filtre. Ouvrir un autre dépôt ou activer un autre onglet repart de l'historique complet. La sélection accepte au plus 64 branches, avec des noms limités à 1 024 octets.

## Actualisation et validation

L'actualisation manuelle et le retour du focus relisent l'état du dépôt actif et les sections ouvertes. Les vues de stashes et de diffs ont des générations et annulations distinctes ; quitter une sélection ou un dépôt libère les caches et ignore les réponses natives arrivées plus tard.

Les tests d'intégration comparent les octets, cibles de symlinks et dates de modification de toutes les entrées des dépôts source, y compris leurs fichiers administratifs. Ils couvrent l'union Solo, les merges, la recherche, la pagination, les modifications externes de références, les worktrees liés et indisponibles, les trois zones de stash et sa suppression externe. Les tests Vue contrôlent les réponses tardives, les listes à la demande et la libération des sélections.

Le benchmark `context_benchmark` utilise un cache applicatif temporaire extérieur au dépôt, conserve seulement des résultats agrégés et mesure 30 lectures de listes, détails et patch de stash, puis 30 alternances entre deux filtres Solo si deux références de branches distinctes existent. Les données, chemins, SHA, branches et auteurs du dépôt source ne sont pas publiés. Les résultats sont dans [performance.md](performance.md) ; les parcours interactifs sont dans [qa-repository-context.md](qa-repository-context.md).

```sh
cargo build --release --locked -p mangrove-core --example context_benchmark
/usr/bin/time -l target/release/examples/context_benchmark /chemin/vers/un/depot
```
