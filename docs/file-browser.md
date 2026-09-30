# Consultation des fichiers

Depuis un fichier modifié du commit sélectionné, la vue centrale propose **Diff**, **Fichier**, **Historique du fichier** et **Blame**. Le chemin et le SHA affichés dans l’en-tête identifient toujours la révision consultée. Changer de vue conserve ces deux valeurs. Choisir un changement dans l’historique ouvre son diff ou son fichier avec le chemin de cette époque, même après un renommage. Le bouton de retour à la révision initiale restaure le fichier du commit sélectionné ; recliquer ce fichier dans la liste redémarre également sa consultation. Un groupe de blame ouvre le message et les métadonnées du commit dans la même vue ; « Voir dans le dépôt » navigue explicitement vers l’historique général.

## Lectures natives

Les commandes `file_content`, `file_history`, `file_blame` et `file_revision_diff` passent par `Workspace` sur le pool bloquant Tauri. Elles utilisent les objets Git du commit demandé, indépendamment des octets présents dans le worktree ou l’index. Les chemins Git conservent une identité hexadécimale sans perte ; le chemin Unicode affiché n’est pas utilisé comme identité. Aucun shell, hook, fetch, édition, stage ou commit n’est exposé par ces commandes.

Les objets manquants donnent une erreur ou une limite explicite : Mangrove ne complète pas un dépôt partiel. Un fichier absent à la révision sélectionnée reste absent ; un bouton permet de consulter explicitement la même adresse dans le premier parent. Un sous-module n’est pas ouvert comme un blob. Un lien symbolique affiche son contenu Git, sans lire sa cible sur disque.

## Contenu et rendu

Les blobs de plus de **2 Mio** ne sont pas décompressés pour le contenu. L’aperçu texte conserve au plus **2 000 lignes** et **256 Kio** ; une ligne qui dépasserait la borne termine l’aperçu avant cette ligne. L’interface annonce la troncature. Les fichiers binaires ou non UTF-8 ne proposent pas de rendu texte ; un fichier vide et l’absence de newline final ont une présentation explicite. Ces limites concernent l’aperçu, pas une exportation du fichier.

La liste de lignes est virtualisée. Les tokens Shiki réutilisent le worker et les limites de syntaxe existants : 20 000 tokens, 256 Ki caractères et 20 Ki caractères par ligne. Les fichiers ordinaires sont analysés depuis leur début ; un dépassement ou un langage inconnu garde le texte brut. Le réglage de police des diffs est partagé avec le contenu et le blame, ainsi que les raccourcis globaux.

## Historique et merges

L’historique remonte le **premier parent uniquement**, comme les diffs de commits. Un merge est donc présenté pour les changements intégrés relativement à ce parent ; les commits propres aux autres parents ne sont pas une seconde chronologie parallèle.

Une page contient au plus **40 changements**. La lecture parcourt au plus **256 commits** et vérifie un délai de **2 secondes entre commits**. Un appel libgit2 individuel peut dépasser ce délai. Une page sans changement peut encore avoir une page suivante. Le suivi de renommages dans l’historique utilise également le contrôle des blobs et candidats avant décompression ; une limite conserve les résultats déjà lus et annonce l’arrêt. Le curseur conserve le commit et le chemin atteints, sans rescanner toutes les pages précédentes. Seule la page affichée est conservée ; le retour arrière garde au plus douze curseurs.

Les renommages suivent la détection de similarité libgit2 déjà utilisée pour les diffs de commits, avec une limite de 200 candidats. Les copies, les changements trop importants et les renommages ambigus ne garantissent pas de continuité. Le parcours s’arrête à une création non reconnue comme renommage. Il ne cherche pas un fichier supprimé puis recréé comme s’il s’agissait forcément du même fichier. Aux merges, le suivi reste sur le premier parent ; un renommage sur un autre parent ne fournit pas une continuité automatique. Une frontière shallow ou un objet absent est signalé.

## Blame

Le blame est calculé à la demande, pour les lignes de l’aperçu, sur le premier parent et au plus **256 commits**. Les groupes indiquent auteur, SHA et date ; une frontière du parcours est marquée. Une attribution de frontière peut provenir d’un commit plus ancien que la borne disponible. Avant calcul, les headers des blobs et des candidats de renommage sont contrôlés : **2 Mio par blob**, **32 Mio de blobs distincts**, **200 candidats de renommage** et **5 000 deltas par révision concernée**. Une borne atteinte arrête le parcours au dernier commit sûr ; la limite est annoncée lorsqu’un groupe atteint cette frontière. La préparation dispose également de deux secondes vérifiées entre commits. Les chemins contenant `*`, `?`, `[` ou `\` ne sont pas utilisables pour le blame : libgit2 les interprète comme des motifs même quand ils désignent un nom littéral, ce qui peut attribuer des lignes au mauvais commit. Le contenu et l’historique restent accessibles. Si une ancienne identité renommée contient ces caractères, le blame s’arrête à cette frontière et annonce sa limite. Les mouvements et copies de lignes entre fichiers ne sont pas suivis. Les bornes du moteur et les objets absents peuvent rendre le blame indisponible.

Le contenu, le patch, le blame et une page d’historique concernent seulement la révision active. Changer de fichier, commit ou dépôt, ou fermer la vue, libère cet état et invalide les réponses en cours. Une réponse native déjà lancée peut finir après l’annulation logique ; elle ne doit jamais remplacer une nouvelle sélection. Aucun blob historique n’est indexé au démarrage.

## Validation

Les tests Rust couvrent modification, renommage, suppression, merge, Unicode, binaire, fichier vide, newline final, pagination et objets absents. Ils comparent aussi les octets et dates de modification de l’ensemble du dépôt source avant et après consultation. Les tests Vue exercent les changements de sélection pendant les lectures et le maintien du chemin et de la révision entre les vues.

Le protocole et les mesures sont dans [performance.md](performance.md). La QA native est consignée dans [qa-file-browser.md](qa-file-browser.md).
