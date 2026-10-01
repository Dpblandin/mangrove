# Profils, connexions et clonage

Les paramètres contiennent les rubriques **Profils**, **SSH** et **Intégrations**. Les profils Personnel et Professionnel existent dès le premier lancement ; d’autres profils peuvent être créés. Chacun conserve ses comptes, ses URLs de serveur, ses favoris, son choix SSH, son thème, ses tailles de texte, ses panneaux, ses récents et son intervalle d’auto-fetch.

Changer de profil annule les lectures distantes, retire leurs pages affichées et revient à un espace local vide. Ouvrir un dépôt local, rechercher des commits ou consulter un diff ne nécessite aucun compte ni réseau. Les lectures d’API et SSH sont explicites ; elles s’exécutent en arrière-plan.

## Parcours

1. Dans Profils, sélectionner un contexte et ajouter éventuellement un chemin local ou une URL aux favoris. Le bouton Ouvrir d’un favori local ouvre le dépôt existant ; un favori distant ne déclenche aucun clone.
2. Dans Intégrations, choisir une édition dans la navigation verticale : GitLab.com ou auto-hébergé, GitHub.com ou Enterprise, Bitbucket Cloud ou Data Center. La fiche affiche les comptes connectés du profil, leur identité disponible et leur serveur. Le formulaire apparaît lors d’un ajout. GitLab et GitHub proposent une connexion navigateur ou un token personnel ; Bitbucket utilise un token. Bitbucket Cloud attend une URL contenant le workspace ; voir [la configuration et les droits minimaux](integrations-providers.md).
3. Connecter vérifie l’utilisateur authentifié pour GitLab/GitHub, ou la première page de dépôts pour Bitbucket sans élargir les permissions, puis conserve les identifiants dans le coffre de l’OS. Aucun dépôt n’est affiché dans les paramètres. Dans **Nouvel onglet → Cloner**, choisir un compte et un dossier parent, puis rechercher et sélectionner un dépôt dans le menu déroulant. Le sélecteur groupe les dépôts par namespace et ajoute les résultats suivants lorsque le bas de la liste devient visible, avec un indicateur de chargement. La recherche filtre les dépôts déjà chargés et continue à chercher dans les pages suivantes quand aucun résultat ne correspond. Le menu, sa position, la recherche et la sélection sont conservés lors de l’ajout. Une erreur interrompt le chargement automatique ; **Réessayer** reprend la page concernée en conservant les résultats. La sélection remplit le nom du nouveau dossier et rend le clonage disponible ; **Consulter les PR/MR** charge leurs données séparément, puis leurs discussions et statuts CI sont consultables dans cet écran dédié.
4. Les PR/MR, commentaires et statuts CI conservent leurs boutons de pagination ; chaque page remplace la précédente. Le chargement progressif des dépôts utilise les mêmes curseurs liés au compte et à la ressource. Les collections restent bornées et un changement de compte ou de profil libère les résultats accumulés.
5. Déconnecter retire le token du coffre et le compte du profil. Un token expiré, révoqué ou privé de droits produit une erreur ; reconnecter avec un token valide après déconnexion. Les erreurs de réseau, de coffre, de permissions et de limite d’API sont affichées au niveau de l’action concernée.

Aucune action ne crée ou modifie une PR/MR, un commentaire, une review, une issue ou un pipeline. Les liens et textes renvoyés par la forge sont des données de lecture ; les adaptateurs REST exposent uniquement GET. Aucun avatar, log CI ni artefact n’est téléchargé automatiquement.

Chaque option affiche le nom court du dépôt sous son namespace. Les titres de namespace restent sur une seule ligne et sont tronqués s’ils dépassent la largeur du menu ; le chemin complet reste utilisé pour la recherche et la valeur sélectionnée. Les projets GitLab marqués pour suppression et ceux dont un segment de chemin porte le suffixe généré `-deletion_scheduled-<ID>` sont exclus du sélecteur, y compris dans un groupe parent en attente de suppression. Les dépôts archivés restent consultables. Voir [le changement GitLab correspondant](https://gitlab.com/gitlab-org/gitlab/-/merge_requests/197431).

## Cloner un dépôt

Depuis **Nouvel onglet → Cloner**, choisir un dépôt du compte ou **URL d’un dépôt** pour saisir une URL HTTPS/SSH. Choisir le dossier parent, le nom du nouveau dossier et le protocole proposé par la forge, puis **Cloner le dépôt**. Le clone s’exécute en arrière-plan et ouvre le dépôt local après réussite ; Échap ou Fermer annule le travail en cours.

Le clonage écrit uniquement dans un dossier neuf réservé atomiquement. Un dossier existant, même vide, est refusé. En cas d’échec ou d’annulation, le dossier partiel est conservé ; sélectionner un autre nom pour réessayer ou gérer ce dossier avec les outils habituels. Mangrove ne supprime aucun dossier de clone.

HTTPS utilise les gestionnaires d’identifiants Git déjà configurés sur la machine, sans invite interactive. SSH utilise l’agent ou la clé existante du profil et exige un hôte déjà reconnu. Les tokens API du coffre Mangrove ne sont pas transmis à Git ; la connexion à une forge ne suffit donc pas à authentifier un clone privé. Les URLs contenant des credentials, une query ou un fragment, les transports locaux et les commandes distantes sont refusés.

Le clone ne lance aucun hook, filtre de checkout ou sous-module et ne récupère pas les objets LFS. Les configurations Git de réécriture d’URL, de proxy et de certificats personnalisés ne sont pas reprises ; seules les configurations de credentials pertinentes sont conservées. Le délai maximal est de quinze minutes, les sorties sont bornées et l’annulation termine Git et ses descendants.

## Connexion GitLab dans le navigateur

La connexion utilise OAuth avec PKCE S256 et le droit `read_api`. Elle ne demande ni mot de passe GitLab ni secret d’application dans Mangrove. Le navigateur système affiche la page de connexion et le consentement GitLab ; le retour rejoint un listener temporaire sur `127.0.0.1`, fermé à la fin du parcours ou à son annulation.

Une application OAuth doit d’abord être enregistrée sur le serveur utilisé. Pour GitLab.com, ouvrir [Applications](https://gitlab.com/-/user_settings/applications), puis **Add new application** :

- **Name** : `Mangrove`.
- **Redirect URI** : exactement `http://127.0.0.1:47321/oauth/callback`.
- **Confidential** : décoché, pour ce client public desktop.
- **Scopes** : uniquement `read_api`.

Après **Save application**, copier **Application ID** dans le champ de Mangrove, puis choisir **Se connecter avec GitLab**. Le champ **Secret** n’est pas utilisé. L’ID désigne l’application OAuth, et non un token personnel ; les utilisateurs d’une même application enregistrée peuvent utiliser son ID. Sur une instance auto-hébergée, enregistrer l’application sur cette instance.

`MANGROVE_GITLAB_CLIENT_ID` préremplit l’ID public au lancement local. Un compte OAuth déjà connecté fournit aussi son ID aux prochaines connexions GitLab.com du même profil. Quand l’ID est configuré, le bouton de connexion reste visible et les instructions sont repliées sous **Configuration OAuth**.

Le renouvellement des tokens OAuth s’effectue avant les lectures quand leur expiration approche ; l’accès et le refresh token restent dans une seule entrée du coffre du compte. Déconnecter supprime cette entrée locale. Pour retirer aussi l’autorisation côté GitLab, révoquer Mangrove dans **Authorized applications** sur GitLab. Mangrove ne génère ni n’ajoute de clés SSH ; il utilise les clés ou agents déjà disponibles.

Les renouvellements concurrents d’un compte sont coordonnés dans le processus Mangrove. La comparaison avant remplacement du coffre protège une déconnexion concurrente dans cette instance ; les coffres OS ne fournissent pas cette garantie entre plusieurs processus. Une annulation réseau ou un échec de stockage après une rotation côté GitLab peut nécessiter une reconnexion.

Sources : [création d’application et droits](https://docs.gitlab.com/integration/oauth_provider/), [PKCE, renouvellement et authentification Bearer](https://docs.gitlab.com/api/oauth2/).

## Connexion GitHub dans le navigateur

Mangrove utilise le **Device Flow** : GitHub s’ouvre dans le navigateur système, puis vous saisissez le code temporaire affiché dans Mangrove et autorisez l’application. Aucun mot de passe ni secret d’application n’est demandé dans Mangrove. Le code peut être copié ou la page GitHub rouverte depuis la carte d’autorisation ; annuler ou changer de contexte termine l’attente et retire ce code de l’écran.

Dans **Intégrations → GitHub.com**, ouvrir la configuration puis **Créer l’application dans GitHub**, ou [New OAuth App](https://github.com/settings/applications/new) :

- **Application name** : `Mangrove`.
- **Homepage URL** : `https://github.com/Dpblandin/mangrove`.
- **Authorization callback URL** : `http://127.0.0.1:47321/oauth/callback`. GitHub demande ce champ lors de l’inscription ; le Device Flow ne l’utilise pas et n’ouvre aucun listener local.
- **Enable Device Flow** : activé.
- **Expire user access tokens** : peut rester activé ; les tokens expirants et leur renouvellement sont pris en charge.

Enregistrer l’application, puis recopier son **Client ID** public dans Mangrove. Le **Client secret** n’est pas utilisé. Choisir **Se connecter avec GitHub**, saisir le code dans GitHub, puis revenir dans Mangrove ; l’identité du compte est vérifiée avant l’enregistrement dans le coffre.

La case **Inclure les dépôts privés** est décochée par défaut. Sans cette option, Mangrove ne demande aucun scope de dépôt privé. Avec elle, l’OAuth App demande `repo` : GitHub accorde aussi des droits d’écriture, même si les adaptateurs Mangrove consultent uniquement les données. Pour des droits limités à la lecture de dépôts choisis, le formulaire de PAT fine-grained reste disponible. Les restrictions d’organisation et SSO continuent de s’appliquer.

`MANGROVE_GITHUB_CLIENT_ID` préremplit le Client ID public au lancement. Un compte GitHub.com OAuth du profil actif peut aussi le fournir. Ces IDs ne sont pas réutilisés pour GitLab ou une instance Enterprise. Pour GitHub Enterprise Server, saisir la racine HTTPS et enregistrer une application sur cette instance, avec Device Flow activé et pris en charge par sa version.

L’attente respecte l’intervalle fourni par GitHub, augmente le délai sur `slow_down` et s’arrête au refus, à l’expiration ou à l’annulation. Les tokens d’accès et de renouvellement éventuels restent dans une seule entrée opaque du coffre, liée au profil, au compte, à l’instance et au Client ID. Les tokens expirants sont renouvelés avant les lectures ; une déconnexion concurrente empêche leur réenregistrement. La révocation côté GitHub se fait dans **Settings → Applications → Authorized OAuth Apps**.

Sources : [création de l’application](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/creating-an-oauth-app), [Device Flow et renouvellement](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps), [scopes OAuth](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps).

## Stockage et secrets

`profiles.json` est dans le répertoire de configuration utilisateur sous `mangrove-gpui`. Il ne contient que des métadonnées : noms et IDs des profils/comptes, fournisseurs, URLs HTTPS, nom d’utilisateur éventuel, méthode d’authentification et ID d’application OAuth, favoris et choix SSH. Les identifiants sont validés, le fichier est borné à 1 Mio et ses écritures sont atomiques. Un fichier invalide n’est pas écrasé par une sauvegarde de profils par défaut.

Les préférences sont dans `preferences-profiles/<id>.json`, à côté du fichier historique `preferences.json`. Le profil Personnel importe les anciennes préférences lors de sa première lecture ; le fichier historique est conservé. Les autres profils démarrent avec les valeurs par défaut. Aucun changement ne réécrit `.gitconfig`, l’identité Git ou les dépôts consultés.

Les tokens utilisent macOS Keychain, Windows Credential Manager ou Secret Service sous Linux, avec une identité distincte par profil et compte. Si le coffre est verrouillé ou indisponible, Mangrove signale l’erreur sans stockage de repli. Les secrets ne sont pas sérialisables, leur Debug est masqué et les réponses ne transmettent pas de token aux vues. Les diagnostics n’incluent ni corps d’erreur serveur ni URL d’authentification. Les réponses contenant un écho identifiable des credentials sont rejetées.

Les POST OAuth servent exclusivement à obtenir et renouveler l’autorisation de connexion ; ils sont séparés des adaptateurs GET de consultation. Les comptes par token créés avant l’ajout d’OAuth restent compatibles.

Les caches distants sont en mémoire et isolés par profil/compte. Le cache de session est borné à 64 entrées et 8 Mio ; les vues conservent les pages actives des PR/MR/commentaires/CI et les dépôts progressivement chargés du compte courant. Cette liste est limitée à 20 000 dépôts et 8 Mio de textes/URLs, en plus des limites de pages du fournisseur ; une limite atteinte est signalée sans supprimer les résultats déjà affichés. Les doublons sont ignorés par ID. Un changement de profil, de compte, de clé SSH ou de lecture invalide les tickets concernés. Les requêtes HTTP utilisent un délai de 20 secondes, TLS avec le magasin de certificats natif, aucune redirection et un maximum de 4 Mio. L’annulation abandonne les futures réseau ; les réponses tardives ne repeuplent pas l’ancien contexte.

Pour une session isolée, définir ensemble `MANGROVE_GPUI_PREFERENCES=/chemin/absolu/preferences.json` et `MANGROVE_PROFILES_PATH=/chemin/absolu/profiles.json`. Ces chemins concernent les métadonnées ; aucun token n’y est enregistré.

## SSH et objets distants

Le modèle retenu est la consultation explicite des branches et tags avec `git ls-remote`. L’agent existant ou une clé existante est choisi par profil ; les hôtes doivent déjà être approuvés. Aucun objet Git n’est récupéré et aucun cache Git distant n’est créé. Le [contrat SSH](integrations-ssh.md) décrit les restrictions, les limites et les tests.

L’auto-fetch déjà fourni pour les dépôts locaux reste une fonction distincte : il utilise les gestionnaires d’identifiants Git et l’agent SSH de la machine. Ses écritures de références/objets dans le dépôt utilisateur constituent l’exception historique documentée. Les tokens et choix SSH des intégrations ne lui sont pas transmis. Les intégrations de cette issue n’ajoutent aucun clone ni fetch implicite.

## Validation et limites

Les [adaptateurs par fournisseur](integrations-providers.md) séparent les éditions implémentées selon leurs documentations et les éditions effectivement validées avec un serveur réel. Les réponses simulées couvrent GitLab, GitHub, Bitbucket Cloud et Server/Data Center ; elles ne certifient pas chaque version auto-hébergée.

La [QA de cette issue](qa-integrations.md) distingue les tests automatisés, le coffre macOS, les parcours natifs et les retours utilisateur. L’utilisateur a confirmé que la connexion GitLab fonctionne après correction de la pagination ; cela ne certifie pas toutes les éditions, la rotation, la révocation ou le clonage privé. Les essais de serveurs SSH et les parcours natifs Windows/Linux restent à réaliser.

La comparaison [des lectures locales](performance-integrations.json) utilise une fixture synthétique de 207 commits, 50 recherches et 50 diffs par condition, avec puis sans requête API simulée en attente. Elle montre que ces lectures terminent pendant la requête distante. Elle ne mesure ni le rendu GPUI, ni un serveur réel, ni une garantie statistique sur un monorepo : les essais natifs avec enrichissement réel restent à faire.

Pour reproduire :

```sh
node scripts/create-fixture.mjs --commits 200 --large-change
cargo run --release --locked -p mangrove-remote --example local_isolation_benchmark -- .mangrove-dev/fixture-200
cargo test --locked -p mangrove-remote
# Test opt-in : identité unique synthétique créée puis supprimée dans le coffre OS.
cargo test --locked -p mangrove-remote os_vault_roundtrip_with_unique_synthetic_identity -- --ignored
```
