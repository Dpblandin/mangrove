# QA profils et intégrations — issue 5

Session du 1 octobre 2026, macOS ARM, Rust 1.95.0. La fixture initiale est générée avec `node scripts/create-fixture.mjs --commits 200 --large-change` : 207 commits au total, dont le commit de démonstration de 46 fichiers. Les préférences et profils de cette première passe utilisent des fichiers temporaires distincts des données utilisateur, sans token utilisateur, dépôt professionnel ni serveur privé. Les ajouts et validations suivants sont détaillés dans leurs sections respectives.

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

## Refonte des connexions et clonage explicite

L’utilisateur confirme que la connexion GitLab fonctionne mieux après la correction précédente et demande une interface de connexions sans liste de dépôts dans les paramètres. Il autorise ensuite explicitement le clonage réel.

La refonte ajoute une navigation verticale des six éditions, des cartes de comptes connectés et un formulaire visible seulement pour une nouvelle connexion. SSH devient une rubrique distincte avec sélecteur de clé existante. L’accueil propose Ouvrir et Cloner ; la navigation des dépôts, les PR/MR et la CI sont déplacées dans un écran dédié. À la demande suivante de l’utilisateur, un Select natif recherchable et groupé par namespace remplace la liste permanente. Le formulaire aligne le dossier parent, le dépôt et le nouveau dossier. Les anciens comptes peuvent actualiser leur identité sans déconnexion ni modification du coffre.

Le backend ajoute la lecture de l’identité GitLab/GitHub, les URLs de clone SSH normalisées et un clonage explicite dans un nouveau dossier. Les tokens API restent séparés des identifiants Git. Les tests utilisent un vrai dépôt synthétique pour vérifier checkout, historique, tag et origin, le refus des destinations existantes et des liens symboliques, la réservation concurrente, l’annulation de Git et de son descendant, les URLs dangereuses et les diagnostics fixes. Après les corrections finales, 74 tests distants passent : 50 unitaires, 23 fournisseurs et un doctest. Les dix tests de clone sont inclus ; le test opt-in du coffre n’est pas relancé dans cette passe. Formatage et Clippy remote/GPUI sans warnings, 84 tests GPUI production et 89 instrumentés passent.

Deux revues `refactor` backend/UI et trois lectures `simplify` parallèles couvrent ce delta. Les corrections rendent la sélection d’un dépôt indépendante des lectures PR/MR, rétablissent Échap depuis les champs, synchronisent le retour aux paramètres et empêchent un clic sur le compte actif d’annuler un clone. Les URLs HTTPS annoncées par les forges partagent désormais le contrat du clone ; les helpers d’environnement Git, d’initiales et de sélecteurs natifs sont réutilisés.

Le delta Select reçoit une revue `refactor` et trois lectures `simplify` supplémentaires. La page ou le compte recrée l’entité et sa subscription : les indices des confirmations différées du kit ne peuvent pas cibler un autre dépôt dans un nouveau jeu de données. Les gardes vérifient aussi le scope et l’entité. Rechoisir la même valeur préserve le nom de dossier personnalisé et les PR/MR ; changer de valeur pendant une lecture MR/CI annule cette lecture. Une confirmation tardive pendant clone/pagination rétablit la sélection correcte. Les fixtures vérifient IDs stables, namespaces et recherche sur le nom complet sans distinction de casse. Les recherches filtrent la page chargée, sans requête réseau ni chargement massif.

Une première passe native avec préférences et profils temporaires vérifie l’accueil Ouvrir/Cloner, les rubriques Profils/SSH/Intégrations, les cartes de comptes et le formulaire GitLab replié. Les comptes synthétiques sans credentials servent uniquement à vérifier l’affichage et le diagnostic de credential absent. L’icône GitLab embarquée remplace le monogramme dans les rails fournisseurs et comptes. Les paramètres n’affichent aucune liste de dépôts. Échap depuis l’URL de clone et le champ SSH revient à l’espace local ; la rubrique SSH est correctement restaurée après ouverture puis fermeture de l’écran distant.

Un clonage HTTPS réel de [octocat/Hello-World](https://github.com/octocat/Hello-World) est déclenché par l’interface vers un nouveau dossier temporaire. Le dépôt s’ouvre automatiquement avec son historique et ses branches : trois commits depuis HEAD, cinq sur toutes les références, `README` contenant `Hello World!`, origin public attendu et working tree propre. Une seconde tentative vers le même dossier affiche le refus de destination existante ; les contenus et dates des treize fichiers du clone restent identiques au snapshot préalable. Ce clone public ne valide pas les credentials HTTPS privés ni un transport SSH réel.

Après l’ajout du Select, le bundle final est reconstruit et relancé avec les mêmes profils temporaires. Le formulaire affiche le dossier parent avant le sélecteur, puis le nouveau dossier ; le sélecteur reste désactivé pour le compte synthétique sans credential. Un nouveau clone public, vers un autre dossier neuf, s’ouvre automatiquement et conserve les mêmes résultats de vérification Git. Le menu peuplé par une forge authentifiée reste à vérifier en essai réel.

Le bundle de QA final utilise les mêmes octets exécutables que le bundle production. SHA-256 : `5eb11a73d4078d6c8b11912e21ddba1eb9c4ccc753233a540c0f6b7031401156`. La fenêtre de validation du clone est quittée par son menu après ces essais.

Les [sept jobs de la correction précédente](https://github.com/Dpblandin/mangrove/actions/runs/36865020055) passent, dont Windows. La refonte et le clonage demandent leur propre validation sur le nouveau commit.

## Chargement progressif dans le sélecteur

À la demande de l’utilisateur, les boutons de pagination des dépôts sont supprimés. Les pages suivantes s’ajoutent en bas du menu ouvert, sans changer la recherche, la sélection ou la position des résultats déjà visibles. Le chargement se déclenche à la peinture du dernier dépôt visible, après vérification du masque de contenu ; la mesure des lignes virtuelles ne lance aucune requête. Une recherche sans correspondance poursuit les pages suivantes. Les erreurs arrêtent l’automatisme et proposent une reprise explicite.

Les nouveaux blocs de namespace sont ajoutés à la fin pour préserver les indices complets et filtrés, y compris lors des confirmations différées du kit. Les doublons sont ignorés par ID et le refus d’une page qui dépasse 20 000 dépôts ou 8 Mio de textes/URLs conserve le catalogue existant. Les callbacks portent le scope du sélecteur ; annulation, tickets et générations empêchent les réponses tardives de repeupler un autre compte ou profil.

Une revue `refactor` puis trois lectures `simplify` parallèles couvrent ce delta. Les corrections vérifient notamment le déclenchement hors mesure/layout, la poursuite d’une première page vide avec un curseur et la réactivation du menu après un clone échoué.

La première observation native sur le compte GitLab existant charge les dépôts authentifiés, puis le défilement déclenche une lecture supplémentaire et ajoute les résultats sans fermer le menu ni déplacer les anciennes lignes. Un titre de namespace très long renvoyé par GitLab se repliait et chevauchait la ligne suivante ; les titres sont désormais tronqués sur une ligne et les options affichent le nom court du dépôt, avec recherche sur le chemin complet. Le suffixe `deletion_scheduled` appartient au chemin fourni par GitLab. À la demande suivante de l’utilisateur, les projets marqués pour suppression et les chemins de projet/groupe parent portant ce suffixe automatique sont exclus de la liste ; aucun dépôt distant n’est modifié ou supprimé par ce filtre.

Après les corrections, 88 tests GPUI production et 93 instrumentés passent, ainsi que Clippy tous targets dans les deux configurations, le formatage et le contrôle du diff. Les dix tests ciblés des intégrations incluent les ajouts stables sous recherche, les doublons, une correspondance trouvée sur une page ultérieure et le refus atomique des limites de nombre/taille.

Le [job Windows du clonage](https://github.com/Dpblandin/mangrove/actions/runs/36871305870/job/110399473250) avait échoué avec `CloneFailed` : la canonicalisation Rust transmet un chemin préfixé `\\?\` que Git for Windows refuse comme argument. Git est désormais lancé dans le dossier neuf réservé et clone vers `.`. Le test réel existant utilise une destination avec espaces à l’intérieur d’un dépôt parent configuré avec réécriture d’URL, hook, helper et filtre factices ; checkout, historique, tag, origin et configuration parent inchangée passent. Les 76 tests remote, Clippy et le formatage passent sur macOS ; le [run `2039703`](https://github.com/Dpblandin/mangrove/actions/runs/36875966391) confirme désormais les sept jobs, dont les tests de clone Windows. Les deux nouveaux tests des fournisseurs vérifient le filtrage des projets/namespaces GitLab en attente de suppression, la conservation du curseur lorsque toute une page est masquée, les archives, les noms proches légitimes et l’absence de filtre sur les autres forges.

La passe native après filtrage GitLab confirme les noms courts et une recherche `deletion_scheduled` sans résultat. La recherche reste saisie pendant puis après la lecture suivante, avec le loader visible dans le menu. Un déplacement du formulaire dû au bouton d’annulation global pendant l’ajout d’une page est corrigé : le chargement supplémentaire reste dans le sélecteur.

Le bundle macOS est reconstruit après ces corrections et relancé avec la configuration OAuth locale. SHA-256 de l’exécutable : `348901cc1c865f2c85e9caa6d6acd610045906ed44ec1502ce097b9ca9496f24`.

## Connexion GitHub par navigateur

L’utilisateur demande également une connexion GitHub dans le navigateur et confirme ne pas encore posséder de Client ID. La configuration d’une OAuth App et l’activation de Device Flow sont donc préparées dans Mangrove ; aucun ID tiers ou secret d’application n’est embarqué. Le parcours affiche un code temporaire, propose de le copier et ouvre la page de vérification de l’instance. L’accès aux dépôts privés se choisit explicitement ; sa portée OAuth GitHub inclut des écritures, tandis que les adaptateurs de consultation restent GET.

Deux revues `refactor` backend/UI, puis trois lectures `simplify` parallèles couvrent ce delta. Les corrections évitent de conserver un ancien code dans les callbacks Copier/Ouvrir, réutilisent la constante de callback affichée et distinguent le timeout réseau d’une vraie expiration du code. Annulation et échéance sont revérifiées après le POST avant de propager son résultat.

Contrôles : 92 tests remote réussis (66 unitaires, 25 fournisseurs et un doctest), dont polling, ralentissement, expiration et annulation, URL/code navigateur hostiles, scopes, tuples de renouvellement, identité du paquet de coffre, rotation concurrente et déconnexion gagnante. Les nouveaux tests de timeout utilisent un POST TLS local sans réponse et une horloge simulée pour la demande initiale, le polling et le renouvellement ; le coffre conserve ses credentials lors d’un timeout réseau. Le test opt-in du coffre OS reste ignoré. GPUI passe 89 tests production et 94 instrumentés ; formatage et Clippy tous targets remote/GPUI dans les deux configurations passent.

QA native sur le nouveau bundle production : le formulaire GitHub.com propose la connexion navigateur, la case privée décochée et la configuration ouverte faute de Client ID. Le repli de la configuration fonctionne ; un clic de connexion sans ID la rouvre avec un diagnostic explicite. Cocher l’accès privé affiche les droits d’écriture inclus, puis décocher restaure l’état public. Le repli PAT et le retour au navigateur fonctionnent. GitHub Enterprise conserve son champ serveur et un Client ID vide distinct ; revenir sur GitLab affiche le compte existant et son formulaire navigateur configuré/replié. Aucun nouvel accès réseau d’authentification ni credential utilisateur n’est utilisé dans ces essais de configuration.

Le packaging macOS release passe. L’application reste ouverte sur GitHub.com, configuration visible et case privée décochée. SHA-256 de l’exécutable : `234169fc51773d16f3ec6be80e158901e3d94b93e9eb736bb30406c2886659bd`.

L’application réelle n’étant pas encore enregistrée, le rendu de la carte avec un code réellement émis, un consentement GitHub réussi, le renouvellement réel et les essais Enterprise restent à effectuer.

## Copie des URLs et feedback d’autorisation

Les URLs de configuration GitHub et la redirection GitLab utilisent le texte sélectionnable fourni par le kit, avec un bouton de copie qui ne reprend pas le libellé. Le feedback **Code copié !** appartient à l’autorisation courante et est réinitialisé lors de la réception d’un nouveau code ; les gardes de génération et le retrait du code après annulation sont conservés. Ce delta local reçoit une relecture directe.

QA native : sélection à la souris de la Homepage URL, ⌘C puis collage dans le champ de nom temporaire donnent exactement l’adresse attendue. Le bouton de callback colle seulement `http://127.0.0.1:47321/oauth/callback`. Le champ temporaire est ensuite vidé. Un compte GitHub OAuth existant est visible après relancement et fournit désormais son Client ID public à la configuration. Une nouvelle demande à GitHub.com émet un code réel ; cliquer **Copier le code** affiche **Code copié !**. L’attente est ensuite arrêtée et la carte du code disparaît ; le compte existant reste affiché. Aucun consentement supplémentaire ni renouvellement réel n’est réalisé par cette passe.

Formatage, Clippy dans les deux configurations, 89 tests GPUI production, 94 instrumentés et packaging macOS passent. Le backend reste inchangé. L’exécutable relancé a pour SHA-256 `f5ba6eb2f49c3b5d2aeefdeded2c8825a9366b55bc16f9a8ab3888f7d227b62d`.

## Essais restants et statut

La PR reste **draft à la demande de l’utilisateur**. Les essais suivants restent à effectuer :

- Connexion réussie, déconnexion et navigation dépôt → PR/MR → commentaires → CI avec un compte de test sur GitLab.com et chaque édition auto-hébergée annoncée, GitHub.com/Enterprise Server et Bitbucket Cloud/Server/Data Center.
- Expiration/révocation et limitation d’API réelles, policies SSO et certificats/proxys d’entreprise sur ces environnements.
- Serveur SSH réel avec identité et clé d’hôte de test déjà autorisées, en mode agent puis fichier ; refus des hôtes inconnus et des clés non autorisées.
- Clones privés réels via les gestionnaires HTTPS de Git et via SSH, sur les éditions annoncées ; comportement sous proxy et avec certificats d’entreprise selon les limites documentées.
- QA native Windows/Linux et coffres natifs de ces plateformes. Les commandes de CI couvrent leurs builds et tests sans prétendre remplacer cette QA.
- Comparaison du rendu natif avec et sans enrichissement authentifié réel. La mesure actuelle couvre uniquement le moteur local et un transport simulé.

Le travail ne ferme pas l’issue et n’annonce pas ces parcours comme validés.
