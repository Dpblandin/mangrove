# Profils et consultation des hébergeurs

Les paramètres contiennent les rubriques **Profils** et **Intégrations**. Les profils Personnel et Professionnel existent dès le premier lancement ; d’autres profils peuvent être créés. Chacun conserve ses comptes, ses URLs de serveur, ses favoris, son choix SSH, son thème, ses tailles de texte, ses panneaux, ses récents et son intervalle d’auto-fetch.

Changer de profil annule les lectures distantes, retire leurs pages affichées et revient à un espace local vide. Ouvrir un dépôt local, rechercher des commits ou consulter un diff ne nécessite aucun compte ni réseau. Les lectures d’API et SSH sont explicites ; elles s’exécutent en arrière-plan.

## Parcours

1. Dans Profils, sélectionner un contexte et ajouter éventuellement un chemin local ou une URL aux favoris. Le bouton Ouvrir d’un favori local ouvre le dépôt existant ; un favori distant ne déclenche aucun clone.
2. Dans Intégrations, choisir GitLab, GitHub, Bitbucket Cloud ou Bitbucket Server, renseigner le nom du compte, son serveur HTTPS et son token. Bitbucket Cloud attend une URL contenant le workspace ; voir [la configuration et les droits minimaux](integrations-providers.md).
3. Connecter vérifie la première page des dépôts accessibles, puis conserve le token dans le coffre de l’OS. Sélectionner le compte charge ses dépôts. Choisir un dépôt charge les PR/MR ouvertes ; choisir une PR/MR permet de consulter ses discussions et les statuts CI du commit source.
4. Chaque bouton de pagination charge la page suivante en remplaçant la page affichée. Les collections sont bornées et les curseurs restent liés au compte et à la ressource. Les commentaires et les statuts CI ont leur pagination propre.
5. Déconnecter retire le token du coffre et le compte du profil. Un token expiré, révoqué ou privé de droits produit une erreur ; reconnecter avec un token valide après déconnexion. Les erreurs de réseau, de coffre, de permissions et de limite d’API sont affichées dans cette rubrique.

Aucune action ne crée ou modifie une PR/MR, un commentaire, une review, une issue ou un pipeline. Les liens et textes renvoyés par la forge sont des données de lecture ; les adaptateurs REST exposent uniquement GET. Aucun avatar, log CI ni artefact n’est téléchargé automatiquement.

## Stockage et secrets

`profiles.json` est dans le répertoire de configuration utilisateur sous `mangrove-gpui`. Il ne contient que des métadonnées : noms et IDs des profils/comptes, fournisseurs, URLs HTTPS, nom d’utilisateur éventuel, favoris et choix SSH. Les identifiants sont validés, le fichier est borné à 1 Mio et ses écritures sont atomiques. Un fichier invalide n’est pas écrasé par une sauvegarde de profils par défaut.

Les préférences sont dans `preferences-profiles/<id>.json`, à côté du fichier historique `preferences.json`. Le profil Personnel importe les anciennes préférences lors de sa première lecture ; le fichier historique est conservé. Les autres profils démarrent avec les valeurs par défaut. Aucun changement ne réécrit `.gitconfig`, l’identité Git ou les dépôts consultés.

Les tokens utilisent macOS Keychain, Windows Credential Manager ou Secret Service sous Linux, avec une identité distincte par profil et compte. Si le coffre est verrouillé ou indisponible, Mangrove signale l’erreur sans stockage de repli. Les secrets ne sont pas sérialisables, leur Debug est masqué et les réponses ne transmettent pas de token aux vues. Les diagnostics n’incluent ni corps d’erreur serveur ni URL d’authentification. Les réponses contenant un écho identifiable des credentials sont rejetées.

Les caches distants sont en mémoire et isolés par profil/compte. Le cache de session est borné à 64 entrées et 8 Mio ; les vues conservent seulement les pages actives. Un changement de profil, de compte, de clé SSH ou de lecture invalide les tickets concernés. Les requêtes HTTP utilisent un délai de 20 secondes, TLS avec le magasin de certificats natif, aucune redirection et un maximum de 4 Mio. L’annulation abandonne les futures réseau ; les réponses tardives ne repeuplent pas l’ancien contexte.

Pour une session isolée, définir ensemble `MANGROVE_GPUI_PREFERENCES=/chemin/absolu/preferences.json` et `MANGROVE_PROFILES_PATH=/chemin/absolu/profiles.json`. Ces chemins concernent les métadonnées ; aucun token n’y est enregistré.

## SSH et objets distants

Le modèle retenu est la consultation explicite des branches et tags avec `git ls-remote`. L’agent existant ou une clé existante est choisi par profil ; les hôtes doivent déjà être approuvés. Aucun objet Git n’est récupéré et aucun cache Git distant n’est créé. Le [contrat SSH](integrations-ssh.md) décrit les restrictions, les limites et les tests.

L’auto-fetch déjà fourni pour les dépôts locaux reste une fonction distincte : il utilise les gestionnaires d’identifiants Git et l’agent SSH de la machine. Ses écritures de références/objets dans le dépôt utilisateur constituent l’exception historique documentée. Les tokens et choix SSH des intégrations ne lui sont pas transmis. Les intégrations de cette issue n’ajoutent aucun clone ni fetch implicite.

## Validation et limites

Les [adaptateurs par fournisseur](integrations-providers.md) séparent les éditions implémentées selon leurs documentations et les éditions effectivement validées avec un serveur réel. Les réponses simulées couvrent GitLab, GitHub, Bitbucket Cloud et Server/Data Center ; elles ne certifient pas chaque version auto-hébergée.

La [QA de cette issue](qa-integrations.md) indique les contrôles automatisés, le coffre macOS et les parcours natifs réalisés. Aucun compte de forge réel ni serveur SSH distant n’est utilisé pour cette validation. Ces essais, ainsi que les parcours natifs Windows/Linux, restent à réaliser avant de présenter l’issue comme entièrement validée.

La comparaison [des lectures locales](performance-integrations.json) utilise une fixture synthétique de 207 commits, 50 recherches et 50 diffs par condition, avec puis sans requête API simulée en attente. Elle montre que ces lectures terminent pendant la requête distante. Elle ne mesure ni le rendu GPUI, ni un serveur réel, ni une garantie statistique sur un monorepo : les essais natifs avec enrichissement réel restent à faire.

Pour reproduire :

```sh
node scripts/create-fixture.mjs --commits 200 --large-change
cargo run --release --locked -p mangrove-remote --example local_isolation_benchmark -- .mangrove-dev/fixture-200
cargo test --locked -p mangrove-remote
# Test opt-in : identité unique synthétique créée puis supprimée dans le coffre OS.
cargo test --locked -p mangrove-remote os_vault_roundtrip_with_unique_synthetic_identity -- --ignored
```
