# Releases

Chaque version de Mangrove est publiée dans les [releases GitHub](https://github.com/Dpblandin/mangrove/releases) avec ses notes, reprises de `CHANGELOG.md`, ses builds, leurs sommes SHA-256 et leurs signatures minisign. Une fois installé, Mangrove se met à jour lui-même : voir [les mises à jour intégrées](#mises-à-jour-intégrées).

## Installer une release

| Fichier | Plateforme |
| --- | --- |
| `mangrove-X.Y.Z-macos-arm64.dmg` | macOS Apple Silicon |
| `mangrove-X.Y.Z-macos-x86_64.dmg` | macOS Intel |
| `mangrove-X.Y.Z-linux-x86_64.tar.gz` | Linux x86_64 avec glibc 2.35 ou plus récente (Ubuntu 22.04, Debian 12, Fedora 36…) |
| `mangrove-X.Y.Z-windows-x86_64.zip` | Windows 10 et 11 x86_64 |
| `SHA256SUMS.txt` | Sommes de contrôle des fichiers ci-dessus |
| `….minisig` | Signature [minisign](https://jedisct1.github.io/minisign/) de chaque archive, vérifiée par les mises à jour intégrées |

Chaque archive contient aussi `LICENSE.md`, la licence de Mangrove, et `THIRD-PARTY-LICENSES.md`, les licences des composants tiers intégrés. Sous Linux, ces deux fichiers sont dans `share/doc/mangrove`.

Sur toutes les plateformes, le fetch et le clonage utilisent Git installé sur la machine.

### macOS

Ouvrir le DMG et glisser **Mangrove** dans **Applications**. Le bundle est signé ad hoc, sans certificat Developer ID ni notarisation : au premier lancement, macOS le bloque. Ouvrir alors **Réglages Système → Confidentialité et sécurité**, puis **Ouvrir quand même**. Il est aussi possible de retirer l’attribut de quarantaine :

```sh
xattr -dr com.apple.quarantine /Applications/Mangrove.app
```

Chaque build a sa propre signature ad hoc : après une mise à jour, macOS peut redemander l’accès aux jetons des intégrations enregistrés dans le trousseau.

### Linux

L’archive contient un préfixe `bin/` et `share/` : l’extraire dans `~/.local` installe l’exécutable `mangrove-gpui`, l’entrée du lanceur et les icônes.

```sh
tar -xzf mangrove-X.Y.Z-linux-x86_64.tar.gz -C ~/.local --strip-components=1
```

`~/.local/bin` doit être dans le `PATH` de la session, ce qui est le cas par défaut sur Ubuntu et Fedora. L’application a besoin de `libxcb`, `libxkbcommon-x11` et d’un pilote Vulkan ; sur Ubuntu : `sudo apt install libxkbcommon-x11-0 libvulkan1 mesa-vulkan-drivers`. Les jetons des intégrations utilisent le Secret Service (GNOME Keyring, KWallet).

### Windows

Extraire l’archive, puis lancer `Mangrove\Mangrove.exe`. L’exécutable n’est pas signé : SmartScreen peut afficher « Windows a protégé votre ordinateur » ; choisir **Informations complémentaires → Exécuter quand même**.

### Mettre à jour ou désinstaller

Une nouvelle version, installée par les [mises à jour intégrées](#mises-à-jour-intégrées) ou à la main, remplace l’application et conserve ses données. Pour désinstaller, supprimer `Mangrove.app` sur macOS, le dossier `Mangrove` sur Windows et, sur Linux, `~/.local/bin/mangrove-gpui`, `~/.local/share/applications/dev.mangrove.gpui.desktop` et les icônes `dev.mangrove.gpui.png` de `~/.local/share/icons/hicolor`.

Les données de Mangrove restent dans ces dossiers :

| Données | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Préférences, profils, favoris, récents et réglages des mises à jour | `~/Library/Application Support/mangrove-gpui` | `~/.config/mangrove-gpui` | `%APPDATA%\mangrove-gpui` |
| Cache des dépôts, reconstruit à la demande | `~/Library/Caches/dev.mangrove.gpui` | `~/.cache/dev.mangrove.gpui` | `%LOCALAPPDATA%\dev.mangrove.gpui` |

Les jetons des intégrations sont dans le coffre de l’OS, sous le service `dev.mangrove.remote` : déconnecter les comptes ou supprimer les profils dans **Paramètres** les efface. Les dépôts consultés ne sont jamais modifiés.

## Mises à jour intégrées

**Paramètres → Mises à jour** affiche la version installée, l’état de la dernière recherche et, pendant un téléchargement, sa progression. Les deux réglages sont actifs par défaut et communs à tous les profils :

- **Rechercher automatiquement** : dix secondes après le lancement, puis toutes les six heures. Mangrove lit la dernière release publique sur l’API GitHub, sans compte ; les brouillons et préversions sont ignorés. **Rechercher les mises à jour**, dans la rubrique ou le menu **Mangrove**, lance une recherche immédiate.
- **Installer automatiquement** : la nouvelle version est téléchargée en arrière-plan, puis installée à la place de l’application. Désactivé, une version trouvée attend le bouton **Installer**.

Une version installée s’ouvre au prochain lancement. Le bouton **Redémarrer**, à côté du numéro de version en bas de la barre latérale ou de l’accueil, ou **Redémarrer maintenant** dans la rubrique, quitte Mangrove et ouvre la nouvelle version ; un clonage en cours demande d’abord confirmation. Une version trouvée mais pas encore installée y est signalée par le bouton **Mise à jour**, qui ouvre la rubrique ; leurs infobulles nomment la version. Quand la barre latérale est trop étroite pour leur libellé, ces boutons se réduisent à leur icône, marquée d’un point pour une version trouvée et d’une coche pour une version installée.

Avant toute installation, l’archive de la plateforme est vérifiée avec sa signature `.minisig` et la clé publique intégrée à Mangrove ; la signature doit aussi porter le nom exact de l’archive, donc sa version et sa plateforme. Une archive modifiée, tronquée, signée par une autre clé ou sans signature n’est jamais installée. La nouvelle version est préparée dans un dossier caché à côté de l’application, puis mise en place par renommage :

| Plateforme | Ce qui est remplacé |
| --- | --- |
| macOS | Le bundle `Mangrove.app` entier, depuis l’image disque montée en lecture seule |
| Linux | Les fichiers de l’archive dans le préfixe où elle a été extraite, `~/.local` par exemple |
| Windows | `Mangrove.exe` et les licences du dossier |

L’ancienne version passe dans un dossier caché `.mangrove-previous-…`, à côté, supprimé à un lancement suivant. Si l’installation échoue, elle est remise en place.

La mise à jour ne remplace que ce qu’elle peut remplacer sans privilèges. Dans ces cas, Mangrove signale la nouvelle version avec un lien vers sa page et l’installation se fait à la main :

- un binaire compilé localement avec `cargo run` ou `cargo build` ;
- une application que macOS exécute depuis une copie temporaire, tant qu’elle n’a pas été déplacée dans **Applications** ;
- une release publiée sans signatures, comme les versions antérieures à cette fonctionnalité ;
- un emplacement en lecture seule ou réservé à un administrateur, comme `/usr/local` sous Linux.

Sur macOS, la protection **Gestion des apps** peut refuser à Mangrove le remplacement de son bundle : l’erreur s’affiche dans la rubrique. Autoriser Mangrove dans **Réglages Système → Confidentialité et sécurité → Gestion des apps**, puis choisir **Installer**, ou installer la version depuis sa page. Chaque build ayant sa propre signature ad hoc, macOS peut aussi redemander l’accès aux jetons du trousseau après une mise à jour.

## Publier une release

La version est commune à l’application et aux crates du moteur. Pendant la série 0.x, une version avec des nouveautés incrémente le deuxième chiffre (0.2.0) ; une version de corrections seules incrémente le troisième (0.2.1).

1. Une seule fois, avant la première release signée, créer la clé de signature des mises à jour et la confier à GitHub. La clé publique est versionnée dans `crates/mangrove-update/release-key.pub` ; la clé secrète reste hors du dépôt, à conserver dans un gestionnaire de mots de passe. La perdre obligerait à publier une nouvelle clé publique, que les versions déjà installées ne connaissent pas : elles ne pourraient plus se mettre à jour seules.

   ```sh
   brew install minisign   # ou le paquet minisign de la distribution
   mkdir -p ~/.minisign
   minisign -G -W -p mangrove-release.pub -s ~/.minisign/mangrove-release.key
   mv mangrove-release.pub crates/mangrove-update/release-key.pub
   gh secret set MINISIGN_SECRET_KEY --repo Dpblandin/mangrove < ~/.minisign/mangrove-release.key
   ```

   `-W` crée une clé sans mot de passe, que le workflow utilise sans invite ; minisign refuse d’écraser une clé secrète existante. Le test `the_release_key_decodes` de `mangrove-update` échoue tant que le fichier ne contient pas de clé publique.
2. Le script génère le changelog avec [git-cliff](https://git-cliff.org/). S’il n’est pas installé, il lance la version 2.14.2 via `npx`, qui demande seulement Node.js.
3. Depuis `main`, à jour et sans modification locale :

   ```sh
   scripts/release.sh 0.2.0
   ```

   Le script refuse une version qui ne suit pas la dernière release et une release sans commit `feat`, `fix` ou `perf`. Il met à jour la version dans `Cargo.toml`, `src-gpui/Cargo.toml` et les deux lockfiles. Il ajoute ensuite en tête de `CHANGELOG.md` la section générée par [cliff.toml](../cliff.toml), puis crée le commit `chore(release): v0.2.0` et le tag annoté `v0.2.0`. Rien n’est poussé.
4. Relire la section avec `git show HEAD -- CHANGELOG.md`. Pour reformuler une entrée, modifier `CHANGELOG.md`, puis :

   ```sh
   git commit --amend --no-edit CHANGELOG.md
   git tag -fa v0.2.0 -m "Mangrove 0.2.0"
   ```

5. Publier :

   ```sh
   git push --atomic origin main v0.2.0
   ```

Le tag lance le workflow [Release](../.github/workflows/release.yml). Il vérifie d’abord que le tag correspond aux manifests, pointe sur `main` et possède une section dans le changelog. Il construit ensuite les quatre archives, les signe avec le secret `MINISIGN_SECRET_KEY`, vérifie chaque signature avec la clé publique du dépôt et publie la release avec la section du changelog. Un secret absent ou une clé qui ne correspond pas arrête la publication. Si un build échoue, aucune release n’est créée : relancer les jobs en échec depuis l’onglet **Actions**.

Les pull requests qui modifient le packaging (workflow, `cliff.toml`, scripts de release ou de packaging, `src-gpui/packaging`, `src-gpui/build.rs`) lancent les mêmes builds sans publier. **Actions → Release → Run workflow** fait de même à la demande. Les archives sont alors téléchargeables dans les artefacts du run.

## Signature

Les archives sont signées avec minisign pour les mises à jour intégrées ; cette signature ne remplace pas celle des systèmes. Les builds macOS sont signés ad hoc et les builds Windows ne sont pas signés. Une distribution sans avertissement demande un certificat Developer ID et la notarisation Apple, qui nécessitent un compte Apple Developer, ainsi qu’un certificat de signature de code pour Windows.
