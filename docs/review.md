# Revue du prototype

Six sous-agents distincts ont appliqué les skills `refactor` et `improve-codebase-architecture` aux frontières ci-dessous. Leurs revues ont été consolidées avant les corrections. Les propositions de refonte sans défaut avéré ont été laissées de côté.

## Lecture Git et conservation du source

L’ouverture du cache pouvait suivre un fichier SQLite symlinké ou hardlinké vers le dépôt source. La garde est maintenant centralisée à chaque ouverture SQLite, avec `SQLITE_OPEN_NOFOLLOW` et contrôles des fichiers et sidecars. Le compteur de liens physiques est vérifié sur Unix ; l’équivalent Windows reste à fournir avant distribution.

L’approfondissement externe d’un clone shallow sans mouvement de refs conservait un ancien historique. L’identité du parcours inclut maintenant les frontières shallow, et les parents effectifs sont recalculés. Une régression vérifie le passage de 1 à 3 commits, les parents et le graphe.

## Index, recherche et graphe du moteur

Une instance du moteur pouvait réutiliser une session annulée après renouvellement du cache par une autre instance. La réutilisation vérifie maintenant l’annulation et la génération persistante.

La recherche SHA utilisait une comparaison SQL parcourant tout l’index. Elle utilise maintenant un préfixe `GLOB` indexable, et cette branche est absente des recherches textuelles. `EXPLAIN QUERY PLAN` et une régression vérifient l’utilisation de l’index.

Le calcul des arêtes traversantes retrouvait la position de chaque voie par un nouveau parcours linéaire. Une table des positions est maintenant construite une fois par ligne. Le contrat et les tests du graphe sont conservés.

## Commandes natives

Les six commandes Rust, leurs arguments et résultats Vue, les permissions et le protocole Tauri embarqué ont été vérifiés. Aucun défaut n’a été confirmé sur cette frontière. Le bridge HTTP facultatif a été retiré pour conserver un produit desktop et un lancement natif par défaut.

Une association TypeScript plus stricte entre chaque commande, ses arguments et son résultat reste une amélioration possible ; elle n’est pas nécessaire aux correctifs de ce prototype.

## État Vue et pagination

Une réponse de page ancienne pouvait réduire le total des résultats après indexation finale, ou laisser une page partielle définitivement en cache. L’acceptation des pages conserve désormais la progression et le total d’une même recherche ; les pages incomplètes sont revalidées même après l’arrêt du polling.

Les régressions vérifient les réponses arrivant dans le mauvais ordre, les pages courtes et les protections déjà présentes pour les ouvertures, recherches, références, détails et diffs.

## Historique virtualisé et clavier

Une flèche clavier vers une page absente perdait son intention de sélection. Le composable gère maintenant la sélection par position et le chargement nécessaire, avec invalidation des anciennes intentions.

Le focus d’une ligne cliquée disparaissait lorsque la virtualisation démontait cette ligne. Le focus reste maintenant sur la liste ; les options utilisent `tabindex=-1` et `aria-activedescendant`. Les tests couvrent les déplacements répétés et leurs annulations. Le parcours est aussi vérifié dans la fenêtre native.

## Détails et outillage

Les statuts transmis par Rust ne correspondaient pas aux initiales attendues par l’interface. Ils sont maintenant définis explicitement, typés côté TypeScript et présentés avec leurs libellés complets ; les tests couvrent ajout, suppression, renommage et changement de type.

Le générateur de fixtures pouvait hériter de variables Git dirigeant ses écritures vers un dépôt extérieur. Il purge ces variables, impose ses chemins Git/worktree, refuse les chemins liés et dossiers inconnus. Les tests comparent les octets et dates des fichiers du dépôt extérieur avant et après génération. Le problème de processus Vite orphelin a disparu avec le retrait du runner HTTP.

## Validation

Les commandes de validation sont dans le README. Elles couvrent le typage, le lint et formatage, 32 tests TypeScript, 16 tests Rust et 6 tests du générateur. Le paquet macOS est construit avec le frontend embarqué. La QA native porte sur l’ouverture d’un dossier, l’indexation, la recherche Unicode et sans résultat, les références, les détails, diffs et la navigation clavier au-delà d’une page de commits. Les validations des autres OS et les intégrations authentifiées restent à réaliser.

## Thèmes et redimensionnement ajoutés ensuite

Les nouvelles préférences de thème et de panneaux ont reçu une revue ciblée complémentaire. Le Canvas relit sa palette lors d’un changement de thème. La fenêtre native est explicitement redimensionnable ; les séparateurs restent dans l’interface Vue, sans ajout de permission ou de commande Git.

La revue a trouvé un débordement quand le panneau historique était réduit dans une grande fenêtre : ses colonnes dépendaient d’une media query sur la fenêtre. Des container queries adaptent maintenant ces colonnes au panneau ; à 340 px, le message conserve 152 px. La QA native a aussi révélé des badges de références qui pouvaient déborder sur l’auteur à 638 px. Les badges sont désormais réductibles, leurs libellés tronqués et la cellule limitée à sa colonne. Les tests de préférences vérifient leur restauration, leurs bornes et l’annulation du redimensionnement.

Les trois palettes, le graphe et les diffs ont été vérifiés dans le paquet natif. Les deux séparateurs répondent au glissement, le séparateur des détails répond aux flèches, et le double clic restaure les tailles par défaut. L’historique a été contrôlé à 340 et 638 px. Une fenêtre réduite de 1440 × 920 à 1200 × 800 conserve la largeur minimale de l’historique ; son agrandissement restaure la largeur souhaitée des détails. Le thème et les largeurs personnalisées persistent après fermeture et relance.
