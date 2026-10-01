# Lecture distante par SSH

Chaque profil possède son propre choix SSH : désactivé, agent SSH existant
(valeur initiale), ou fichier de clé privée existant choisi par l’utilisateur.
Mangrove ne génère aucune clé, ne lance aucun agent et n’enregistre ni clé ni
passphrase. Seul le chemin de la clé sélectionnée est stocké dans le profil.

La lecture est déclenchée explicitement et exécute `git ls-remote --heads --tags`
sur l’URL SSH. Le résultat contient les noms de branches et de tags et leurs
identifiants d’objets annoncés, y compris les cibles des tags annotés. Aucun objet
Git n’est téléchargé : pas de clone, de fetch, de miroir, de cache d’objets ni de
modification des références d’un dépôt local. Les références renvoyées restent
en mémoire pour l’affichage. Ce service est indépendant du fetch déjà présent
dans le moteur des dépôts locaux.

## Préparation

- Installer Git et OpenSSH et les rendre accessibles à l’application.
- Pour le mode agent, utiliser un agent déjà démarré avec l’identité autorisée
  déjà chargée. Sur Unix, `SSH_AUTH_SOCK` doit désigner un socket existant.
- Pour le mode fichier, sélectionner un fichier existant par son chemin absolu.
  Une clé chiffrée nécessitant une saisie de passphrase ne fonctionne pas dans
  ce mode ; charger cette clé dans l’agent et sélectionner le mode agent.
- Vérifier préalablement la clé d’hôte et l’ajouter à son fichier `known_hosts`
  au moyen des outils SSH habituels. Mangrove n’accepte ni n’ajoute une nouvelle
  clé d’hôte automatiquement et refuse une clé d’hôte modifiée.
- Utiliser un nom d’hôte réel dans une URL `ssh://git@host:2222/team/repo.git` ou
  une adresse de type `git@host:team/repo.git`. Les alias définis dans
  `~/.ssh/config`, les proxys SSH et les commandes SSH personnalisées ne sont
  pas utilisés. Les chemins distants acceptés sont des chemins ASCII simples
  sans espace, encodage `%`, paramètres d’URL ni métacaractères de shell.

Le jeton GitHub ou GitLab du profil sert exclusivement à l’API HTTPS du service.
La clé SSH sert exclusivement au transport Git SSH. Une connexion API réussie
ne garantit donc pas un accès SSH au dépôt, et une clé SSH autorisée ne remplace
pas un jeton API.

## Isolation et limites

Git est lancé directement, sans shell pour ses arguments, depuis un répertoire
temporaire vide supprimé à la fin. Il n’utilise aucune configuration Git du
dépôt appelant, de l’utilisateur ou du système. Les variables `GIT_*` héritées,
dont les commandes SSH et les destinations de traces, sont supprimées puis
remplacées par les seuls paramètres nécessaires à la lecture. La configuration
SSH personnelle est ignorée ; OpenSSH conserve la vérification des fichiers
`known_hosts` existants. Les options SSH imposent le mode batch, interdisent les
prompts, les commandes locales et l’ajout automatique de clés d’hôte. Le chemin
de clé est cité comme un unique argument lorsque Git invoque sa commande SSH.

La durée maximale est de 30 secondes et la sortie de références est limitée à
4 Mio. Les deux sorties du processus sont drainées simultanément pour éviter
qu’un pipe plein bloque Git. Une annulation ou un dépassement du délai arrête
Git et ses descendants SSH. Les diagnostics affichés sont des messages fixes :
ils ne reprennent jamais la sortie du serveur, une URL, un chemin de clé ou une
valeur secrète.

L’agent, le fichier de clé et les fichiers de confiance appartiennent à
l’utilisateur et doivent être disponibles au processus de l’application. Git
et OpenSSH manquants, un hôte inconnu, un refus d’authentification et un réseau
injoignable produisent une erreur sans dialogue de saisie. Le premier support
est vérifié sur macOS ; la terminaison des groupes de processus est également
prévue sur Unix et la terminaison des arbres de processus sur Windows.

## Validation

`cargo test --locked -p mangrove-remote ssh::tests` couvre les URL autorisées et
les tentatives d’injection, le quoting des chemins de clés, le parsing SHA-1 et
SHA-256, les branches et tags annotés, les sorties bornées, le drainage des deux
pipes, les diagnostics sans secrets et l’arrêt des descendants après timeout
ou annulation. Un vrai `git ls-remote` lit un dépôt bare synthétique local et
compare tous ses chemins et contenus avant/après. Une fixture supplémentaire
utilise une URL localhost et un faux exécutable SSH relié au même type de dépôt
synthétique pour vérifier les arguments réels envoyés par Git, y compris un
chemin de clé contenant des métacaractères. Ces tests ne contactent aucun
service distant et ne nécessitent aucune clé utilisateur.

Un test de bout en bout avec un serveur SSH réel exige un serveur de test, une
clé dédiée déjà autorisée et sa clé d’hôte déjà vérifiée. Il peut utiliser une
URL SSH localhost vers un dépôt bare synthétique : comparer le contenu du dépôt
avant/après, vérifier les références affichées, puis vérifier séparément le
refus d’un hôte absent de `known_hosts`, d’une clé non autorisée et d’une clé
chiffrée sans agent. Aucun serveur SSH n’est démarré automatiquement par la suite
de tests.
