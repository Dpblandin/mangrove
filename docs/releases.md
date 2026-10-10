# Releases

Chaque version de Mangrove est publiée dans les [releases GitHub](https://github.com/Dpblandin/mangrove/releases) avec ses notes, reprises de `CHANGELOG.md`, ses builds et leurs sommes SHA-256.

## Installer une release

| Fichier | Plateforme |
| --- | --- |
| `mangrove-X.Y.Z-macos-arm64.dmg` | macOS Apple Silicon |
| `mangrove-X.Y.Z-macos-x86_64.dmg` | macOS Intel |
| `mangrove-X.Y.Z-linux-x86_64.tar.gz` | Linux x86_64 avec glibc 2.35 ou plus récente (Ubuntu 22.04, Debian 12, Fedora 36…) |
| `mangrove-X.Y.Z-windows-x86_64.zip` | Windows 10 et 11 x86_64 |
| `SHA256SUMS.txt` | Sommes de contrôle des fichiers ci-dessus |

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

Installer une nouvelle version remplace l’application et conserve ses données. Pour désinstaller, supprimer `Mangrove.app` sur macOS, le dossier `Mangrove` sur Windows et, sur Linux, `~/.local/bin/mangrove-gpui`, `~/.local/share/applications/dev.mangrove.gpui.desktop` et les icônes `dev.mangrove.gpui.png` de `~/.local/share/icons/hicolor`.

Les données de Mangrove restent dans ces dossiers :

| Données | macOS | Linux | Windows |
| --- | --- | --- | --- |
| Préférences, profils, favoris et récents | `~/Library/Application Support/mangrove-gpui` | `~/.config/mangrove-gpui` | `%APPDATA%\mangrove-gpui` |
| Cache des dépôts, reconstruit à la demande | `~/Library/Caches/dev.mangrove.gpui` | `~/.cache/dev.mangrove.gpui` | `%LOCALAPPDATA%\dev.mangrove.gpui` |

Les jetons des intégrations sont dans le coffre de l’OS, sous le service `dev.mangrove.remote` : déconnecter les comptes ou supprimer les profils dans **Paramètres** les efface. Les dépôts consultés ne sont jamais modifiés.

## Publier une release

La version est commune à l’application et aux crates du moteur. Pendant la série 0.x, une version avec des nouveautés incrémente le deuxième chiffre (0.2.0) ; une version de corrections seules incrémente le troisième (0.2.1).

1. Le script génère le changelog avec [git-cliff](https://git-cliff.org/). S’il n’est pas installé, il lance la version 2.14.2 via `npx`, qui demande seulement Node.js.
2. Depuis `main`, à jour et sans modification locale :

   ```sh
   scripts/release.sh 0.2.0
   ```

   Le script refuse une version qui ne suit pas la dernière release et une release sans commit `feat`, `fix` ou `perf`. Il met à jour la version dans `Cargo.toml`, `src-gpui/Cargo.toml` et les deux lockfiles. Il ajoute ensuite en tête de `CHANGELOG.md` la section générée par [cliff.toml](../cliff.toml), puis crée le commit `chore(release): v0.2.0` et le tag annoté `v0.2.0`. Rien n’est poussé.
3. Relire la section avec `git show HEAD -- CHANGELOG.md`. Pour reformuler une entrée, modifier `CHANGELOG.md`, puis :

   ```sh
   git commit --amend --no-edit CHANGELOG.md
   git tag -fa v0.2.0 -m "Mangrove 0.2.0"
   ```

4. Publier :

   ```sh
   git push --atomic origin main v0.2.0
   ```

Le tag lance le workflow [Release](../.github/workflows/release.yml). Il vérifie d’abord que le tag correspond aux manifests, pointe sur `main` et possède une section dans le changelog. Il construit ensuite les quatre archives et publie la release avec la section du changelog. Si un build échoue, aucune release n’est créée : relancer les jobs en échec depuis l’onglet **Actions**.

Les pull requests qui modifient le packaging (workflow, `cliff.toml`, scripts de release ou de packaging, `src-gpui/packaging`, `src-gpui/build.rs`) lancent les mêmes builds sans publier. **Actions → Release → Run workflow** fait de même à la demande. Les archives sont alors téléchargeables dans les artefacts du run.

## Signature

Les builds macOS sont signés ad hoc et les builds Windows ne sont pas signés. Une distribution sans avertissement demande un certificat Developer ID et la notarisation Apple, qui nécessitent un compte Apple Developer, ainsi qu’un certificat de signature de code pour Windows.
