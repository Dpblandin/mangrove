# QA profils et intégrations — issue 5

Session du 1 octobre 2026, macOS ARM, Rust 1.95.0. La fixture est générée avec `node scripts/create-fixture.mjs --commits 200 --large-change` : 207 commits au total, dont le commit de démonstration de 46 fichiers. Les préférences et profils utilisent des fichiers temporaires distincts des données utilisateur. Aucun token utilisateur, dépôt professionnel ni serveur privé n’est utilisé.

## Contrôles automatisés

- Moteur Git : formatage, Clippy tous targets et 70 tests réussis. Les tests de lecture seule comparent le contenu et les dates de modification du dépôt source ; le moteur local n’a pas été modifié par cette issue.
- Intégrations : formatage, Clippy tous targets, 27 tests de profils/session/transport/SSH, 13 tests des quatre fournisseurs et le doctest de non-sérialisation des secrets passent. Couverture des comptes identiques dans des profils distincts, des réponses tardives, de l’annulation effective, des caches bornés, de la pagination, des credentials expirés/révoqués, des limites d’API et des échos de secrets dans les corps, liens et curseurs.
- SSH : Git réel sur dépôt bare synthétique et transport SSH simulé localhost, avec snapshot avant/après, chemins de clés contenant des métacaractères, sorties bornées et arrêt des descendants après annulation/timeout. Aucune clé utilisateur ni connexion SSH réelle.
- Coffre macOS : test opt-in réussi. Une identité de test unique contient un secret synthétique ; création, lecture, isolation et suppression sont vérifiées. Le test nettoie son entrée sans toucher les comptes existants.
- GPUI : 81 tests production, dont préférences par profil, refus des réponses obsolètes et déconnexion tardive scoppée au profil initial. La configuration instrumentée passe 86 tests et Clippy avec `app-benchmark`.
- Générateur de fixture : 8 tests réussis. Harness de mesure : 7 tests réussis, 3 tests de contrat Linux ignorés sur macOS.

Commandes :

```sh
cargo fmt --all --check
cargo clippy --locked -p mangrove-core --all-targets -- -D warnings
cargo test --locked -p mangrove-core
cargo clippy --locked -p mangrove-remote --all-targets -- -D warnings
cargo test --locked -p mangrove-remote
cargo test --locked -p mangrove-remote os_vault_roundtrip_with_unique_synthetic_identity -- --ignored
cargo fmt --manifest-path src-gpui/Cargo.toml --all --check
cargo clippy --locked --manifest-path src-gpui/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-gpui/Cargo.toml
cargo clippy --locked --manifest-path src-gpui/Cargo.toml --features app-benchmark --all-targets -- -D warnings
cargo test --locked --manifest-path src-gpui/Cargo.toml --features app-benchmark
node --test scripts/create-fixture.test.mjs
python3 -B scripts/benchmark-ui.test.py
sh src-gpui/scripts/package-macos.sh
```

## QA native

La première passe sur le bundle release observe les rubriques Général, Profils et Intégrations, les deux profils initiaux et les formulaires des quatre fournisseurs. Un favori local ajouté au profil Professionnel disparaît lors du passage au profil Personnel et revient lors du retour au profil Professionnel. L’ouverture de ce favori charge le dépôt synthétique ; la recherche `fixture` retourne les deux commits de démonstration et le diff de `useDiff.ts` affiche ses trois lignes ajoutées, sans connexion à un compte.

Le profil Professionnel conserve un thème Macchiato et le profil Personnel retrouve sa palette Classique. La première passe révèle que le texte du sélecteur de thème gardait encore l’ancien choix malgré la palette restaurée ; cette synchronisation a été corrigée avant la passe finale.

La saisie d’un token synthétique invalide dans le formulaire GitHub n’expose aucune valeur dans l’arbre d’accessibilité. Connecter efface le champ, puis affiche le diagnostic fixe « Identifiants expirés, révoqués ou permissions insuffisantes ». Aucun compte authentifié n’est ajouté et aucun contenu privé n’est consulté. Ce scénario vérifie le refus d’authentification ; il ne valide pas un parcours de connexion réussi.

La passe finale relance le binaire production dans un bundle de QA à identifiant distinct, avec des octets exécutables identiques. SHA-256 de l’exécutable : `ba3922d870de69c9a4bbbfb7f494fdae2608f00733ec38fd849a880ad6e1c46a`. Après relancement, les favoris professionnels sont conservés et absents du profil personnel. Le passage Professionnel → Général affiche « Catppuccin Macchiato » ; Personnel → Général affiche « Classique », avec leur palette et leur intervalle propres. La synchronisation du sélecteur est donc vérifiée sur le binaire final. La fenêtre de QA est ensuite quittée par son menu.

## Revues et performance

Deux passes `refactor` déléguées couvrent le backend et l’interface. La passe `simplify` délègue trois lectures en parallèle : réutilisation, qualité et efficacité. Les corrections portent notamment sur la déconnexion achevée après changement de profil, les échos de secrets encodés, la borne cumulée des commentaires et les sauvegardes/timers évités lors d’une simple modification de favoris ou de compte.

Le [rapport de comparaison du moteur](performance-integrations.json) mesure 50 recherches et 50 diffs par condition. Les p95 observés sont environ 0,83/0,79 ms pour la recherche et 1,48/1,26 ms pour les diffs, sans/avec une API synthétique en attente. L’annulation simulée prend environ 0,09 ms. Les valeurs montrent que les opérations locales terminent pendant la lecture distante ; elles ne constituent pas une estimation statistique de l’impact réseau ni du rendu natif.

## Essais restants et statut

La PR reste **draft à la demande de l’utilisateur**. Les essais suivants restent à effectuer :

- Connexion réussie, déconnexion et navigation dépôt → PR/MR → commentaires → CI avec un compte de test sur GitLab.com et chaque édition auto-hébergée annoncée, GitHub.com/Enterprise Server et Bitbucket Cloud/Server/Data Center.
- Expiration/révocation et limitation d’API réelles, policies SSO et certificats/proxys d’entreprise sur ces environnements.
- Serveur SSH réel avec identité et clé d’hôte de test déjà autorisées, en mode agent puis fichier ; refus des hôtes inconnus et des clés non autorisées.
- QA native Windows/Linux et coffres natifs de ces plateformes. Les commandes de CI couvrent leurs builds et tests sans prétendre remplacer cette QA.
- Comparaison du rendu natif avec et sans enrichissement authentifié réel. La mesure actuelle couvre uniquement le moteur local et un transport simulé.

Le travail ne ferme pas l’issue et n’annonce pas ces parcours comme validés.
