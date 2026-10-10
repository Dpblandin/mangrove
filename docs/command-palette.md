# Palette de commandes

**⌘P** sur macOS ou **Ctrl+P** sur Windows/Linux ouvre la palette depuis l’accueil, le dépôt ou les paramètres, même lorsque le focus est dans un champ. Le menu **Affichage → Palette de commandes…** offre le même accès. Une nouvelle invocation réinitialise la recherche dans la palette existante.

La saisie filtre immédiatement les libellés, catégories, mots-clés et chemins. Elle ignore la casse et les accents, y compris les accents Unicode décomposés. Tous les mots saisis doivent correspondre ; un libellé exact ou commençant par la recherche précède une correspondance dans les mots-clés. Les alias anglais courants sont acceptés, notamment `settings`, `theme`, `search`, `fetch` et `clone`.

**↑ / ↓** déplacent la sélection et la révèlent dans la liste. **Entrée** ou un clic exécute une commande disponible. **Échap** ou un clic sur le fond ferme la palette sans changer la consultation et rend le focus au contrôle précédent, ou à l’historique si aucun contrôle n’avait le focus. Les raccourcis sont indiqués au format de la plateforme : ⌘O sur macOS, Ctrl+O sous Windows/Linux. **Tab / Maj+Tab** gardent le focus dans la recherche ; les actions globales de l’espace de travail sont interceptées pendant l’ouverture. Les raccourcis d’édition du champ restent utilisables.

## Commandes disponibles

- **Dépôts et onglets** : ouvrir un dépôt local, nouvel onglet sur l’accueil, ouvrir ou retirer chaque dépôt récent, cloner ou consulter un dépôt distant, choisir un onglet avec son raccourci Cmd/Ctrl + 1…9, passer à l’onglet suivant ou précédent, fermer l’onglet actif, quitter l’application. Le chemin distingue les dépôts ou worktrees portant le même nom.
- **Git** : historique du dépôt, recherche de commits, modifications locales, actualisation, fetch manuel, filtre des branches et tags, ouverture des sections worktrees et stashes. Les worktrees et stashes déjà chargés sont également proposés individuellement ; ouvrir leur section déclenche leur lecture habituelle à la demande.
- **Commit sélectionné** : copie du SHA, navigation vers chacun des parents, ajout/retrait du mode Solo pour chacune des branches locales ou distantes pointant sur ce commit. La sortie du mode Solo est proposée lorsqu’il est actif.
- **Fichier explicitement sélectionné** : diff, contenu, historique et blame pour une révision de commit ; diff pour les modifications locales et stashes, conformément aux vues existantes. Choix entre le diff central et compact ; fermeture de la vue centrale. Le chargement ou un aperçu automatique du premier fichier ne suffit pas à rendre les commandes de fichier disponibles.
- **Paramètres** : accès à la vue actuelle et aux rubriques Général, Profils, SSH et Intégrations ; activation/désactivation des aperçus d’images et de l’auto-fetch. Réactiver l’auto-fetch depuis la palette fixe l’intervalle à une minute ; son réglage précis reste dans Général.
- **Apparence** : choix direct de Classique, Mangrove, Catppuccin Macchiato, Ardoise et Gitlab, avec indication du thème actif ; zoom global et remise à zéro, taille du texte des diffs, réinitialisation des largeurs des panneaux gauche et droit. Les changements suivent la persistance du profil actif et mettent à jour les paramètres.
- **Messages** : fermeture de l’erreur de l’espace de travail lorsqu’elle est présente.

Les actions sans dépôt, sans modification locale ou sans remote affichent leur indisponibilité. Les opérations de lecture en cours et les sélecteurs natifs conservent leurs gardes. Avant exécution, la commande est comparée au catalogue actuel : une fermeture d’onglet, un changement de dépôt, de commit ou de fichier ne peut pas rediriger un clic en attente vers une ancienne sélection.

## Parcours conservés dans leurs vues

Les opérations qui demandent plusieurs valeurs ou une confirmation passent par leur écran existant : clonage, création/édition des profils, connexions aux fournisseurs, configuration SSH et consultation des PR/MR, discussions et CI. La palette ouvre directement la rubrique ou le navigateur de dépôts approprié. Elle ne contourne pas leurs validations, leurs confirmations ou le coffre de l’OS.

La sélection d’une ligne de commit ou de fichier, l’expansion d’un dossier de branches, la pagination et les défilements restent dans leurs listes ; filtrer les références ou ouvrir une section donne accès à ces parcours. Le redimensionnement libre reste sur les séparateurs ; la palette offre leur remise à zéro. Copier/sélectionner le texte du code ou d’un champ conserve les commandes d’édition habituelles. Ces interactions ne sont pas dupliquées comme commandes indépendantes.

La palette reprend les fonctionnalités de Mangrove ; elle n’ajoute pas de checkout, de staging, de commit, ni d’application/suppression de stash.

## Organisation et vérification

`src-gpui/src/command_palette.rs` possède la recherche, la sélection, les états accessibles et le rendu virtualisé. `src-gpui/src/app/commands.rs` construit le catalogue à partir de l’état actif et appelle les opérations existantes. Les générations du dépôt et de la sélection accompagnent les commandes contextuelles ; les observations des vues mettent à jour une palette ouverte.

Les tests de la palette couvrent les accents, les correspondances partielles, le classement, les recherches vides, la sélection après filtrage/actualisation et le rejet des commandes périmées ou désactivées. Les 125 tests de l’application passent, ainsi que `cargo fmt --check` et Clippy avec `-D warnings`.

La vérification native sur macOS utilise une instance isolée et un dépôt de démonstration avec modifications locales, stash et worktree lié. Elle couvre l’ouverture depuis l’espace de travail, l’accueil, les paramètres et le menu ; la recherche, les flèches, Entrée, Échap, Tab/Maj+Tab, la réinitialisation de la recherche et la fermeture par clic sur le fond ; le retour du focus et le blocage des raccourcis sous la palette ; les quatre thèmes et leur persistance ; les quatre rubriques des paramètres ; les vues contenu et blame d’un fichier, la consultation d’un stash, l’ouverture d’un worktree dans un onglet, le navigateur de dépôts distants et les commandes indisponibles sans dépôt. Le parcours de création d’onglet est revérifié après correction du transfert du focus.

Le raccourci Ctrl+P est enregistré pour Windows/Linux, mais l’exécution et le rendu natifs sur ces deux plateformes restent à vérifier. Les connexions aux fournisseurs et le clonage réel ne sont pas exécutés dans cette vérification.
