# Architecture du prototype

## Application native

Mangrove est une application Tauri 2. Vue 3 et Tailwind CSS 4 composent son interface dans la WebView système. Vite+ sert à installer les dépendances, développer, vérifier, tester et compiler les assets embarqués. Le moteur Git et SQLite tournent dans le processus Rust natif. Aucun serveur HTTP n’est requis en production.

## Contrat de lecture

`Workspace` est le module central du moteur. Il expose six opérations : ouvrir un dépôt, connaître la progression d’indexation, lire une page de commits, retrouver la position d’un commit, lire ses détails et lire un diff de fichier. L’adaptateur Tauri appelle ce contrat ; les tests et benchmarks l’exercent indépendamment de la fenêtre native.

Le moteur utilise `git2` / libgit2, avec les transports réseau désactivés. Il n’exécute pas de commande Git ni de hook. Il ne modifie ni le worktree, ni l’index Git, ni les refs, ni la configuration Git. Les permissions Tauri exposent uniquement les six commandes de lecture et le sélecteur de dossier ; aucun accès shell ou filesystem général n’est accordé à l’interface.

Les paramètres et caches propres à l’application peuvent être écrits. Le cache SQLite est placé dans le cache utilisateur de l’OS, hors des dépôts consultés. Le moteur rejette un emplacement de cache contenu dans le worktree, les dossiers Git ou le worktree principal d’un worktree lié, y compris via un lien symbolique. L’ouverture du fichier SQLite et de ses sidecars refuse aussi les symlinks et, sur Unix, les hardlinks. Les tests vérifient ces règles et la conservation des fichiers source. Le contrôle équivalent des hardlinks Windows devra être ajouté et vérifié avant une distribution sur cet OS.

La consultation des modifications non commitées, stashes et contenus complets de fichiers n’est pas encore fournie. La vue actuelle porte sur les commits accessibles depuis HEAD et les références locales.

## Chargement progressif

Une ouverture photographie HEAD et les références, puis lance un parcours topologique. Deux workers d’indexation sont partagés par `Workspace`, avec au plus huit sessions de dépôt. Les commits sont persistés par lots de 256 ; les lecteurs voient des snapshots SQLite cohérents.

Les métadonnées textuelles immuables de commits et leur index FTS5 sont conservés. Un changement de références ou de frontières shallow renouvelle le parcours et son graphe sans reconstruire les métadonnées déjà connues. Les parents effectifs sont relus, car l’approfondissement d’un clone shallow peut les changer sans déplacer les refs. Une génération et l’annulation des anciens travaux empêchent une réponse d’une ancienne ouverture de remplacer la nouvelle. Plusieurs instances du moteur partageant un cache utilisent aussi une garde de génération, vérifiée avant réutilisation d’une session.

L’interface demande des pages de 128 lignes et conserve au plus douze pages. La virtualisation limite les éléments DOM aux lignes visibles et à une marge de huit lignes. Le graphe est peint sur Canvas avec un espacement fixe des voies et un déplacement horizontal pour les graphes larges. Dans les résultats de recherche, les liens entre commits non adjacents sont masqués pour éviter un graphe trompeur.

## Recherche et détails

FTS5 indexe messages, auteurs et emails, avec tokenisation Unicode et préfixes de mots. SHA et noms de références ont une recherche par préfixe. La recherche n’est pas une recherche floue, une recherche de code ni une indexation de tous les blobs historiques. La saisie est temporisée de 160 ms, et les réponses obsolètes sont ignorées.

Les fichiers et patches sont lus uniquement à la sélection. Un merge est comparé à son premier parent. L’affichage est borné : 5 000 fichiers, blobs de 2 Mio, patches de 2 000 lignes ou 512 Kio ; l’interface annonce les données binaires et les résultats tronqués. Ces bornes limitent la taille des réponses ; le calcul Git sous-jacent doit encore être mesuré sur les merges et arbres extrêmes.

## Préférences d’interface

Les trois thèmes utilisent les mêmes tokens CSS, y compris les couleurs du Canvas et des diffs. Le thème Classique, gris avec accents bleus, est le défaut ; Mangrove conserve la palette verte, et Macchiato reprend la palette officielle Catppuccin. Le choix est restauré avant le montage de l’interface. Un changement recolore le graphe sans rouvrir le dépôt.

Le module `usePaneLayout` conserve les largeurs préférées des panneaux branches et détails dans les préférences de l’application. Les dimensions affichées sont bornées selon la fenêtre, avec un historique d’au moins 340 px ; réduire la fenêtre ne détruit pas la préférence pour une fenêtre plus grande. Les séparateurs proposent capture du pointeur, annulation, clavier et réinitialisation. Les colonnes d’historique utilisent des container queries, car la largeur d’un panneau peut changer indépendamment de la fenêtre.

## Intégrations à venir

GitHub, GitLab et Bitbucket auront des adaptateurs de lecture distincts, y compris leurs variantes auto-hébergées. Les profils seront des préférences de Mangrove, sans synchronisation vers `.gitconfig`. Les secrets devront rester dans le coffre de l’OS. Le SSH devra réutiliser les clés et agents existants ; l’ajout de clés chez un fournisseur est exclu du périmètre de lecture seule.

L’authentification peut nécessiter une requête POST d’échange de token ; la règle de lecture seule concerne les opérations métier sur les dépôts et fournisseurs, pas seulement les verbes HTTP. Clone et fetch éventuels devront être limités à un cache de dépôt appartenant à Mangrove, jamais au dépôt source sélectionné. Ils ne sont pas implémentés dans ce prototype.

## Choix différés

Le remplacement de libgit2 par `gix`, WebGL et une recherche de contenu historique ne sont pas engagés sans mesures sur des dépôts représentatifs. Le benchmark initial est une référence reproductible, pas une garantie de performances sur tous les monorepos.
