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

## Ajout de la connexion navigateur GitLab

La passe suivante ajoute OAuth PKCE S256, avec une application publique et `read_api`, tout en conservant la connexion par PAT. Deux implémentations ont été parallélisées, suivies de revues `refactor` backend/UI et d’une passe `simplify` avec trois lectures parallèles. Les corrections incluent un diagnostic spécifique pour le port local occupé, la protection du refresh token dans les données normalisées et le retrait des accès bruts inutiles de l’API publique.

Contrôles après les corrections : formatage et Clippy remote/GPUI sans warnings, 40 tests unitaires remote, 13 tests fournisseurs et 1 doctest, puis 82 tests GPUI production et 87 instrumentés. Le test opt-in du coffre macOS valide aussi la comparaison avant remplacement, la persistance d’une rotation et l’absence de recréation après suppression, avec une identité synthétique nettoyée.

Les tests OAuth couvrent le challenge du RFC PKCE, l’état aléatoire, le callback loopback réel avec Host/path/paramètres stricts, les refus et bornes, l’annulation et le timeout des sockets, les scopes, la compatibilité des anciens comptes PAT, la rotation concurrente, l’échec de stockage et la déconnexion pendant le renouvellement. Les vérifications de fermeture observent EOF/reset sur une connexion détenue par le test ; un premier probe sur un port éphémère libéré pouvait échouer si un autre test recyclait ce port, et a été remplacé.

QA native macOS : le bouton **Se connecter avec GitLab** est visible sans instructions de création lorsque l’ID est configuré ; le panneau de configuration révèle l’ID public prérempli, puis se replie. Le repli PAT et le formulaire GitHub restent disponibles. Le bundle production final a été relancé avec l’ID public fourni par l’utilisateur, sans inscrire cet ID dans les sources, et la rubrique Intégrations est affichée. SHA-256 de l’exécutable : `ebd31c7eda90f5362af1b9addc0a5b040ad9770be236c1d995fdd70ece46bbaf`.

Cette passe n’a pas effectué le consentement utilisateur ni une connexion GitLab authentifiée réelle. Le retour navigateur, les lectures authentifiées, la persistance après relancement, le renouvellement réel et la déconnexion restent à confirmer avec l’application enregistrée. La coordination du coffre protège les opérations du processus ; elle ne garantit pas une comparaison atomique entre plusieurs processus Mangrove.

## Correction de la pagination GitLab réelle

Le premier essai utilisateur après cette passe affiche « Le serveur a fourni une pagination non sûre ». Ce diagnostic vient de la validation des liens pendant la lecture des projets, avant l’enregistrement de la connexion dans le coffre. Cet essai ne valide donc pas une connexion persistante réussie.

Des requêtes publiques sans credentials reproduisent la forme des liens : GitLab.com ajoute ses paramètres par défaut à `/projects` et les paramètres `id` de route aux MR et pipelines. Les discussions nécessitent une authentification ; leurs paramètres `id` et `noteable_id` sont confirmés dans la [définition API GitLab](https://gitlab.com/gitlab-org/gitlab/-/blob/master/lib/api/discussions.rb#L36-57), sans essai authentifié. Le [générateur de liens](https://gitlab.com/gitlab-org/gitlab/-/blob/master/lib/gitlab/pagination/offset_header_builder.rb#L54-61) fusionne ces paramètres avec ceux de pagination.

La correction accepte seulement les valeurs par défaut propres à ces quatre endpoints et les identifiants strictement identiques au chemin. Elle conserve les refus des filtres modifiés ou supprimés, des paramètres inconnus ou dupliqués, des échos de secrets et des pages trop grandes. Trois tests supplémentaires couvrent trois pages successives, GitLab.com et une instance à sous-chemin, puis les variantes malveillantes. Le test reproduisant les liens réels échoue avec `UnsafePagination` avant la correction et passe après celle-ci.

Formatage, Clippy distant tous targets et 57 tests backend passent : 40 tests unitaires, 16 tests des fournisseurs et un doctest. Le test opt-in du coffre reste ignoré dans cette commande habituelle ; il avait passé lors de la validation OAuth précédente. La revue `refactor` puis les trois lectures `simplify` déléguées sur ce delta n’ont trouvé aucun défaut supplémentaire.

Le [job Windows de la passe OAuth](https://github.com/Dpblandin/mangrove/actions/runs/36863673846/job/110373777788) échouait dans le test de timeout avec `overflow when subtracting duration from instant`. Le calcul utilise désormais une échéance absolue et une durée restante saturée ; le test positionne l’échéance à maintenant, sans soustraire 300 secondes à l’horloge Windows. La limite totale d’attente reste de cinq minutes. Le test ciblé et les 13 tests OAuth passent localement ; la correction Windows reste à confirmer dans le prochain run CI.

Clippy GPUI tous targets et le packaging macOS release passent. Le bundle reconstruit est relancé sur Intégrations, avec GitLab.com et l’ID public configurés localement. SHA-256 de l’exécutable : `7eeb1e33721d9a42320c70d39db922088139363c1c5a21b1a7d32dc2f94d6e6b`. Le nouvel essai authentifié reste à effectuer par l’utilisateur ; cette passe de diagnostic n’a utilisé aucun credential réel.

## Essais restants et statut

La PR reste **draft à la demande de l’utilisateur**. Les essais suivants restent à effectuer :

- Connexion réussie, déconnexion et navigation dépôt → PR/MR → commentaires → CI avec un compte de test sur GitLab.com et chaque édition auto-hébergée annoncée, GitHub.com/Enterprise Server et Bitbucket Cloud/Server/Data Center.
- Expiration/révocation et limitation d’API réelles, policies SSO et certificats/proxys d’entreprise sur ces environnements.
- Serveur SSH réel avec identité et clé d’hôte de test déjà autorisées, en mode agent puis fichier ; refus des hôtes inconnus et des clés non autorisées.
- QA native Windows/Linux et coffres natifs de ces plateformes. Les commandes de CI couvrent leurs builds et tests sans prétendre remplacer cette QA.
- Comparaison du rendu natif avec et sans enrichissement authentifié réel. La mesure actuelle couvre uniquement le moteur local et un transport simulé.

Le travail ne ferme pas l’issue et n’annonce pas ces parcours comme validés.
