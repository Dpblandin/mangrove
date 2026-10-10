# Worktrees, stashes et mode Solo

La sidebar GPUI consulte ces données par les méthodes Rust/git2 de `Workspace`, en arrière-plan. La consultation ne change ni HEAD, ni les fichiers, ni l'index, ni les références, ni la configuration des dépôts. Aucun checkout, création/suppression de worktree ou application/suppression de stash n'est exposé.

## Worktrees

La section Worktrees se charge à son ouverture et présente une entrée par branche, ou HEAD détachée avec son SHA court, suivie de son chemin atténué où le dossier personnel est abrégé en `~`. Le worktree principal porte une icône de maison ; le worktree ouvert prend la couleur d'accent de la branche courante et est coché. Le survol affiche le chemin complet du répertoire de travail, le type de worktree et les éventuels motifs de verrouillage ou d'indisponibilité. Ces informations sont aussi incluses dans le libellé accessible. Les entrées verrouillées ou indisponibles conservent leurs icônes d'état. Un verrou Git interdit certaines opérations de maintenance ; il n'empêche pas la consultation d'un worktree disponible. Les inscriptions dont le dossier manque restent affichées sans réparation ou nettoyage.

Le champ de filtre de l'espace de travail s'applique aux références et aux worktrees chargés. Il recherche une portion du nom de branche ou du libellé de HEAD détachée, SHA court compris, sans tenir compte de la casse ni des espaces autour de la recherche. Les portions correspondantes sont surlignées dans les dossiers de références, les branches, les tags et les worktrees, y compris lorsqu'une recherche traverse plusieurs segments de leur nom. Le compteur Worktrees indique le nombre de résultats ; une section ouverte sans résultat affiche « Aucun worktree trouvé. ». Effacer le filtre retire les surlignages et réaffiche la liste complète chargée.

Ouvrir une entrée disponible utilise son dossier de travail dans le parcours d'onglets existant. Une entrée déjà ouverte active cet onglet. Chaque worktree conserve son identité de session et son index Git local ; les gardes existantes interdisent un cache applicatif dans le worktree, ses dossiers Git ou le worktree principal. Un dépôt bare indique explicitement qu'il ne possède pas de répertoire de travail.

À l'ouverture d'un dépôt ou à l'activation d'un onglet, l'historique sélectionne le commit de HEAD du worktree et défile jusqu'à lui, même s'il se trouve au-delà de la première page ou doit encore être indexé. Le graphe complet reste partagé entre les branches. L'actualisation conserve le commit choisi ; en Solo, elle reste dans le parcours des branches isolées.

La réponse contient au plus 256 entrées, principal compris, et annonce sa troncature. Les lectures de noms et inscriptions sont natives ; elles ne lancent aucun processus Git ou shell.

## Onglets et dépôts récents

Les onglets et les dépôts récemment ouverts utilisent le même nom lisible : le nom du dépôt principal, ou « dépôt · branche » pour un worktree lié. Une HEAD détachée inclut son SHA court. Les infobulles conservent le nom complet et le chemin source. Le survol ou le focus remplit tout l'onglet, zone de fermeture comprise. Le dépôt principal d'un worktree lié est identifié par Git ; un dossier Git principal séparé doit renseigner `core.worktree` pour que Git retrouve son répertoire de travail externe.

À l'accueil ou dans un nouvel onglet, une lecture native bornée aux six récents actualise leurs anciens noms de dossiers sans ouvrir de session ni lancer d'indexation. Un dépôt manquant conserve son dernier nom connu. Le bouton de retrait enlève seulement l'entrée de cet historique local, sans fermer les onglets ni toucher au dépôt ; rouvrir le dossier l'ajoute à nouveau. Une réponse de métadonnées tardive ne réinsère pas un élément retiré et ne remplace pas le nom d'une ouverture plus récente. Le survol ou le focus remplit toute la carte du dépôt récent.

## Stashes

La section Stashes se charge à son ouverture. Elle présente au plus les 128 entrées les plus récentes du reflog de `refs/stash`, avec leur référence `stash@{n}` et leur message sans le préfixe « On branche: » ou « WIP on branche: » ; une ligne atténuée indique la branche d'origine, la date et l'auteur. Le survol ajoute la date complète et le commit de base. La troncature est annoncée. Un stash est identifié par son SHA, indépendamment de son numéro `stash@{n}`, qui peut changer après une opération externe.

La sélection charge ses listes de fichiers ; le patch et sa coloration se chargent lorsque l'utilisateur ouvre un fichier. Les trois comparaisons ont des onglets distincts :

- **Répertoire de travail** : arbre du deuxième parent (index sauvegardé) → arbre du commit de stash. Affiche les changements non indexés sauvegardés.
- **Index** : arbre du premier parent (commit de base) → arbre du deuxième parent. Affiche les changements indexés sauvegardés.
- **Non suivis**, seulement si le stash possède un troisième parent : arbre vide → arbre de ce troisième parent.

Le panneau indique le sens de la comparaison et conserve ses identifiants de révision. Un fichier modifié avant et après indexation peut apparaître dans les deux premières zones, avec deux diffs différents. Les renommages sont détectés ; les clés opaques des chemins conservent les noms Git non UTF-8. Le backend vérifie que le SHA demandé appartient encore à la liste consultable du reflog avant de lire détails ou diff : un stash supprimé extérieurement ne reste pas accessible par une ancienne sélection.

Les listes de fichiers, patches, blobs et couleurs utilisent les mêmes limites que les autres diffs : 5 000 fichiers par zone, fichiers de 2 Mio, 2 000 lignes ou 512 Kio par patch, cache et coloration bornés. Les limites de réponses ne bornent pas toutes les allocations internes de libgit2 : la lecture du reflog, la comparaison des arbres et la détection des renommages restent à mesurer sur des cas extrêmes.

Les commits techniques de stash restent exclus des racines de l'historique général et de la recherche. Consulter les stashes n'indexe pas leurs instantanés dans le graphe normal.

## Solo

Le bouton Solo d'une branche locale ou distante est accessible dans sa ligne de sidebar à la souris ; au clavier, Maj + F10 ou la touche Menu ouvrent la même action depuis la branche active de l'arbre. Le menu contextuel de la sidebar et le clic droit sur un badge de branche dans le graphe proposent la même action. Le clic droit sur une ligne de commit propose toutes les branches locales ou distantes attachées à ce commit, même celles dont le badge n'est pas affiché, pour choisir explicitement la référence à isoler. Les tags et les commits sans branche attachée ne proposent pas de Solo. Maj + F10 ou la touche Menu ouvrent aussi ces actions depuis le commit sélectionné.

Le menu propose « Solo », « Ajouter au Solo » ou « Retirer du Solo » selon le filtre actuel. Une branche déjà sélectionnée est retirée ; retirer la dernière restaure l'historique complet. Plusieurs branches forment l'union de leurs historiques. Le bandeau indique les branches sélectionnées et permet de quitter le mode. Échap ferme le menu et rend le focus à son origine ; le défilement du graphe ou une actualisation des références ferme les actions périmées. Ouvrir le menu n'altère pas la sélection du commit.

Le moteur parcourt tous les commits accessibles depuis les références sélectionnées, avec tous les parents des merges et les ancêtres communs dédupliqués. Il recalcule les positions et les voies du graphe pour ce parcours. HEAD n'est pas ajouté comme racine lorsque le Solo est actif ; le nom de la branche réellement active reste visible dans l'en-tête. Les modifications locales restent celles du worktree ouvert.

La pagination, la recherche et la position des commits utilisent le même parcours. Une recherche textuelle conserve le comportement existant : les liens entre résultats non adjacents sont masqués. Les métadonnées et l'index FTS5 sont réutilisés ; changer le Solo renouvelle seulement le parcours et le graphe dans le cache applicatif du worktree. Les réponses des anciennes générations ne peuvent pas remplacer la nouvelle vue.

Le clic normal sur une branche conserve la navigation vers son dernier commit. Si ce commit est hors du parcours Solo, le filtre est quitté pour permettre cette navigation. Le Solo est conservé lors d'une actualisation du dépôt actif, puis recalculé depuis les références actuelles ; les branches supprimées extérieurement quittent le filtre. Ouvrir un autre dépôt ou activer un autre onglet repart de l'historique complet. La sélection accepte au plus 64 branches, avec des noms limités à 1 024 octets.

## Actualisation et validation

L'actualisation manuelle et le retour du focus relisent l'état du dépôt actif et les sections ouvertes. `RepositoryContextState` conserve une lane physique et une dernière demande par liste : une lecture native engagée termine avant que la demande suivante parte, et seule la génération courante publie son résultat. Les vues de stashes et de diffs ont des générations distinctes ; quitter une sélection ou un dépôt libère les caches concernés et ignore les réponses natives arrivées plus tard.

Un stash sélectionné est aussi relu lors de l'actualisation si la section est repliée. Son absence dans une liste complète provoque un événement `StashUnavailable` ; l'application vérifie encore le dépôt et la sélection avant de retirer ses détails et de revenir à un commit. Une erreur de lecture ou une liste tronquée ne prouve pas sa disparition et ne retire pas la sélection.

Les tests d'intégration du moteur comparent les octets, cibles de symlinks et dates de modification de toutes les entrées des dépôts source, y compris leurs fichiers administratifs. Ils couvrent l'union Solo, les merges, la recherche, la pagination, les modifications externes de références, les worktrees liés et indisponibles, les trois zones de stash et sa suppression externe. Les tests GPUI protègent les demandes de listes remplacées, les réponses obsolètes, l'absence de stash dans une liste complète ou tronquée, le tri, les replis et la navigation au clavier de l'arbre, ainsi que les tickets de noms récents.

Le benchmark `context_benchmark` utilise un cache applicatif temporaire extérieur au dépôt, conserve seulement des résultats agrégés et mesure 30 lectures de listes, détails et patch de stash, puis 30 alternances entre deux filtres Solo si deux références de branches distinctes existent. Les données, chemins, SHA, branches et auteurs du dépôt source ne sont pas publiés. Les résultats sont dans [performance.md](performance.md). La QA GPUI est décrite dans le [README natif](../src-gpui/README.md) ; [qa-repository-context.md](qa-repository-context.md) conserve les parcours de l'ancienne interface.

```sh
cargo build --release --locked -p mangrove-core --example context_benchmark
/usr/bin/time -l target/release/examples/context_benchmark /chemin/vers/un/depot
```
