<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.craft-anything.png" width="128" height="128" alt="Icône Craft Anything : un corbeau gravé sur un carreau bleu">
</p>

<h1 align="center">Craft Anything</h1>

<p align="center">
  <a href="README.md">English</a> · <a href="README.ja.md">日本語</a> · <a href="README.zh.md">简体中文</a> · <a href="README.zh-hant.md">繁體中文</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <b>Français</b> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.pt.md">Português</a> · <a href="README.th.md">ไทย</a> · <a href="README.tr.md">Türkçe</a>
</p>

<p align="center">
  <b>Découvrez comment n’importe quel site web, application ou jeu fonctionne, puis laissez l’IA construire votre propre version ; un moteur open-source, en salle blanche, écrit en pur Rust.</b><br>
  Pointez-le vers une cible, dites ce que vous voulez, et il étudie l’original avec de vrais outils d’inspection,<br>
  fait construire un nouveau projet par un agent de codage, compare le résultat à l’original et l’affine.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux · FreeBSD · Web" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20FreeBSD%20%C2%B7%20Web-native-2d81ff?style=flat-square">
  <img alt="Licence : MIT OR Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Statut : alpha précoce" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Rejoignez la communauté ArtCraft sur Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="#downloads"><b>Télécharger Craft Anything</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">Toutes les applications Crafting</a>
</p>

<br>

<p align="center">
  <img src="docs/images/project-website.png" alt="Un projet de site web dans Craft Anything : les contrôles indiquent ce qui ne correspond pas encore et le degré de ressemblance de chaque page, le constructeur dit que sa version est prête, une carte d’approbation demande avant d’exécuter une commande, et le plan montre deux étapes sur quatre terminées" width="100%">
  <br>
  <sub>Un projet de site web : chaque contrôle indique le degré de ressemblance de la construction, vous approuvez tout ce qui sort des règles, et le plan montre ce qui reste à faire.</sub>
</p>

> [!NOTE]
> **ArtCraft est une communauté d’artistes de tous horizons.** Peintres, photographes,
> cinéastes, illustrateurs, designers, animateurs, amateurs et personnes qui ont pris un crayon
> la semaine dernière. Si vous créez des choses, vous faites partie de nous. **[Venez dire bonjour sur Discord](https://discord.gg/artcraft).**

<p align="center">
  <a href="#how-it-works">Comment ça marche</a> ·
  <a href="#two-ways-to-use-it">Deux façons de l’utiliser</a> ·
  <a href="#ai">IA</a> ·
  <a href="#built-for-agents">Agents</a> ·
  <a href="#under-the-hood">Sous le capot</a> ·
  <a href="#get-started">Pour commencer</a> ·
  <a href="#status">Statut</a> ·
  <a href="#downloads">Téléchargements</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3><a id="-study-anything"></a>🔍 Étudier n’importe quoi</h3>
      Sites web, applications de bureau, applications mobiles, jeux et fichiers : de vrais outils d’inspection découvrent comment ils sont construits et ce qu’ils font, avec les preuves derrière chaque réponse.
    </td>
    <td width="25%" valign="top">
      <h3><a id="-build-your-own"></a>🛠️ Construire le vôtre</h3>
      Dites ce que vous voulez (« Recréer ce site web », « Recréer le gameplay de ce jeu en Rust ») et un agent de codage construit un nouveau projet dans la stack que vous demandez.
    </td>
    <td width="25%" valign="top">
      <h3><a id="-checked-not-guessed"></a>✅ Vérifié, pas deviné</h3>
      Chaque construction est comparée à l’original (pages à trois largeurs d’écran, auto-test d’un jeu, exécutions d’une application) et affinée jusqu’à ce qu’elle soit proche.
    </td>
    <td width="25%" valign="top">
      <h3><a id="-agent-ready"></a>🤖 Prêt pour les agents</h3>
      320 outils, chacun étant une commande CLI et un outil MCP, afin que n’importe quel agent puisse piloter le même moteur que l’application utilise. Aucune IA n’est nécessaire pour inspecter.
    </td>
  </tr>
</table>

<br>

## <a id="how-it-works"></a>Comment ça marche

Pointez Craft Anything vers une cible, dites ce que vous voulez, et appuyez sur Démarrer. Il fonctionne en quatre étapes,
en vous demandant en mots simples quand une décision vous appartient :

1. **Étudier :** inspecter l’original : ses pages à chaque largeur, son code, ses fichiers, comment il s’exécute.
2. **Construire :** un agent de codage construit un nouveau projet, dans le langage, le framework ou le moteur que votre
   requête nomme (ou un choix par défaut raisonnable).
3. **Vérifier :** comparer la construction à l’original et lister ce qui ne correspond pas encore.
4. **Prêt :** ouvrir, jouer ou prévisualiser le résultat, ou demander des modifications et il l’affine.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/make.png" alt="L’écran Make : choisissez un site web, une application ou un jeu à recréer, tapez l’adresse, choisissez Salle blanche ou Approfondi, dites ce que vous voulez et appuyez sur Démarrer" width="100%"><br>
      <sub><b>Make :</b> choisissez ce qu’il faut recréer, dites ce que vous voulez, appuyez sur Démarrer.</sub>
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/project-game.png" alt="Un projet de jeu de serpent terminé avec toutes ses étapes faites et un bouton Jouer au jeu" width="100%"><br>
      <sub><b>Prêt :</b> jouez au jeu, ou demandez des modifications ci-dessous.</sub>
    </td>
  </tr>
</table>

## <a id="two-ways-to-use-it"></a>Deux façons de l’utiliser

- **Simple** (par défaut) : « Que voulez-vous créer ? » Choisissez un site web, une application ou un jeu, décrivez-le,
  appuyez sur Démarrer. Une page de projet affiche les quatre étapes (Étudier → Construire → Vérifier → Prêt), ce que
  l’agent fait en mots simples, des cartes de question et d’approbation, le degré de ressemblance du résultat, et
  des boutons pour ouvrir le résultat ou son dossier. Demandez des modifications dans la zone en bas.
  « Juste regarder à l’intérieur » ouvre une cible pour des questions sans rien construire (en mode Approfondi).
- **Avancé** : l’établi complet : chaque outil (Ctrl/Cmd+K), fonctions, chaînes, pseudo-C,
  désassemblage, hex, graphes d’appels, preuves, inconnues, tâches, logs et la console de l’agent.

Les deux modes pilotent le même moteur.

## <a id="clean-room-or-deep"></a>Salle blanche ou Approfondi

- **Salle blanche** (par défaut) : rien à l’intérieur de l’original n’est lu, ni son code, ni ses fichiers, ni le source des pages.
  Craft Anything travaille à partir de ce que vous pouvez voir : les images que vous ajoutez, le site web tel qu’un visiteur
  le voit (captures d’écran, texte visible, mise en page mesurée, couleurs, polices et espacements, menus ouverts)
  et, lorsque vous l’autorisez, une image de la fenêtre de l’application. Les outils qui regarderaient à l’intérieur sont refusés
  et indiquent comment activer le mode Approfondi.
- **Approfondi** : chaque outil est utilisé, pour votre usage privé. Le code et les fichiers d’un programme sont lus avec les
  moteurs intégrés (et Ghidra lorsqu’il est installé), le JavaScript d’un site web est lu et sa page de démarrage est observée,
  et le constructeur peut les utiliser tous.

Le mode Salle blanche est une règle qui porte sur ce que Craft Anything propose et refuse. Ce n’est pas un bac
à sable au niveau du système d’exploitation : votre ordinateur n’est pas isolé. Un agent de codage que
vous lancez agit toujours en votre nom, et ce qu’il peut lire, écrire ou exécuter dépend de ses propres
permissions. Les outils propres à un agent que Craft Anything peut refuser varient selon l’agent ; pour
les outils de Claude Code dans les conversations et les questions, voir
[`docs/claude-code.md`](docs/claude-code.md).

Basculez dans la barre de titre ou dans Réglages, avec `settings_set deep_mode true`, ou pour une exécution CLI avec
`--deep` (ou `--clean-room`).

<p align="center">
  <img src="docs/images/workbench.png" alt="L’établi Avancé : une liste de fonctions, le pseudo-C de la fonction sélectionnée, l’agent répondant à une question avec un appel d’outil, et le registre des preuves" width="100%">
  <br>
  <sub><b>Avancé :</b> fonctions, pseudo-C, l’agent et les preuves derrière chaque réponse.</sub>
</p>

## <a id="ai"></a>IA

La construction nécessite un agent de codage sur votre ordinateur. **Claude Code** est pris en charge en premier : Craft
Anything exécute votre propre programme `claude` non modifié, qui utilise votre propre connexion ; Craft Anything
ne voit ni ne gère jamais votre compte. Votre utilisation relève de votre propre accord avec
Anthropic. Voir [docs/claude-code.md](docs/claude-code.md) pour le comment et le pourquoi. Un adaptateur Codex
existe mais n’a pas encore été vérifié sur une installation réelle. **GitHub Copilot** peut utiliser votre
Copilot CLI installé via ACP : connectez-vous séparément avec `copilot login`. D’autres agents qui parlent ACP (Gemini CLI,
Qwen Code, opencode, goose, Hermes, Maki) peuvent être basculés vers ACP, backend par backend ; parmi ceux-ci, seul Maki a
été exécuté en direct. Anthropic, OpenAI, OpenRouter et les serveurs locaux
compatibles OpenAI sont disponibles pour les questions et l’analyse (Réglages → IA).

## <a id="built-for-agents"></a>Conçu pour les agents

Chaque outil est également une sous-commande CLI et un outil MCP, afin qu’un agent puisse étudier une cible ou exécuter
une reconstruction complète sans l’application :

```sh
craft-anything-cli commands                                   # every tool, by toolset
craft-anything-cli mcp                                        # serve them to any MCP client
craft-anything-cli rebuild https://example.com "Recreate this website"
```

Les résultats sont des enregistrements de preuves avec leur confiance, couverture et limitations ; les résultats volumineux sont d’abord renvoyés sous forme de résumé et sont paginés avec `core_evidence_browse`. `craft-anything-cli setup`
connecte Craft Anything aux applications d’agents de codage.

## <a id="under-the-hood"></a>Sous le capot

Un moteur de rétro-ingénierie entièrement écrit en Rust, avec 320 outils (300 dans le moteur, le reste provenant des couches IA et
setup) pour les programmes natifs, .NET, Java, Android, Apple, Electron/JavaScript, sites web,
firmware, captures de paquets, bases de données et fichiers de jeux. Ses propres décodeurs, désassembleur et
décompilateur fonctionnent sans aucun programme externe ; le mode Approfondi les utilise tous, et Ghidra lorsqu’il est
installé. Il alimente l’application de bureau (egui), le CLI et le serveur MCP, et des parties s’exécutent dans
le navigateur en tant que WebAssembly. La liste complète se trouve dans [docs/tools.md](docs/tools.md).

## <a id="get-started"></a>Pour commencer

Nécessite Rust **1.97 ou plus récent** (une dépendance l’exige ; `rustup update`). Sur Debian ou Ubuntu,
l’application de bureau a également besoin de :

```sh
sudo apt-get install build-essential libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev libgtk-3-dev
```

Puis :

```sh
git clone https://github.com/storytold/craft-anything && cd craft-anything
cargo run --release -p craft-anything                # the desktop app
cargo install --path apps/craft-anything-cli         # the CLI, as `craft-anything-cli`
craft-anything-cli doctor                            # checks the install, coding agents and engines
```

La construction nécessite un agent de codage connecté sur votre ordinateur (voir [IA](#ai)) ; `doctor` indique ce qui
manque. Reconstruction depuis la ligne de commande (les questions sont posées dans le terminal) :

```sh
craft-anything-cli rebuild https://example.com "Recreate this website"
craft-anything-cli rebuild - "A snake game in Rust" --rounds 1
craft-anything-cli rebuild https://example.com "Make the header blue" --continue-in "<project folder>"
craft-anything-cli rebuild - "Add a contact form" --project-folder "<your folder>" --existing edit
```

Les nouveaux projets vont dans `Documents/Craft Anything` (modifiable dans Réglages → Général). Chaque dossier de projet
contient `craft.json` (la requête, la cible, le statut et l’historique), `capture/` (ce qui a été
enregistré depuis la cible), `app/` (le projet généré), `checks/` (chaque comparaison contre
l’original), `investigation.cany/` (preuves) et `inbox/` (questions ouvertes).

Un projet peut également démarrer dans un dossier que vous choisissez (Choisir un dossier… sous Démarrer, ou `--project-folder`).
S’il contient déjà des fichiers, vous indiquez ce qu’ils sont : l’application elle-même, que le constructeur édite sur place,
ou du matériel de départ, avec l’application construite dans `app/` à côté d’eux (Réglages → Général peut
mémoriser la réponse). Réglages → Général liste également les projets d’autres dossiers ; en retirer un
ne fait que le retirer de la liste.

## <a id="status"></a>Statut

> [!IMPORTANT]
> **Statut :** Craft Anything est en alpha précoce. Ce qui fonctionne aujourd’hui, ce qui est partiel et ce qui vient ensuite :
>
> - **Fonctionne aujourd’hui :** l’application de bureau (Simple et Avancé), le CLI et le serveur MCP ; le
>   flux de travail Étudier → Construire → Vérifier → Affiner pour les sites web, applications et jeux avec Claude Code comme
>   constructeur ; vérification des sites web sur trois largeurs d’écran ; reconstructions de jeux en Unity, Unreal ou Godot ; mode Approfondi
>   avec Ghidra ; images et modèles 3D générés (opt-in, votre propre clé fal.ai) ; plugins (serveurs MCP, et composants WASM dans les builds avec l’option `wasm-plugins`).
> - **Partiellement fonctionnel :** construction avec des clés API et d’autres agents de codage (construits, pas encore exécutés en direct),
>   contrôles de comportement pour les applications et les jeux, quelques outils du moteur testés uniquement sur des substituts.
> - **Publié :** les versions publiques avec installateurs pour Windows, macOS, Linux et FreeBSD sont
>   publiées sur la [page Releases](https://github.com/storytold/craft-anything/releases) ; elles sont
>   toujours en alpha.
> - **Ensuite :** le niveau managé.
>
> [docs/STATUS.md](docs/STATUS.md) contient le détail et comment chaque partie a été vérifiée,
> [docs/tool-status.md](docs/tool-status.md) la checklist outil par outil, et
> [docs/ROADMAP.md](docs/ROADMAP.md) le plan, point par point. Merci de signaler les issues, ou de nous dire ce qui
> a cassé sur [Discord](https://discord.gg/artcraft).

**Utilisation responsable.** Utilisez Craft Anything sur des logiciels que vous possédez ou que vous êtes autorisé à examiner. Il ne contourne jamais les DRM, les logins ou les anti-cheat. Les reconstructions recréent la mise en page, le style et le comportement à partir des preuves ;
il est demandé à l’agent de ne pas copier les logos, photos ou textes à moins que vous ne confirmiez que vous les possédez ou avez
la permission.

**Installer des dépendances exécute du code.** Lorsqu’un constructeur installe les dépendances d’un
projet, les commandes s’exécutent sur votre ordinateur, et les paquets peuvent exécuter du code pendant
leur installation. npm exécute par défaut les scripts d’installation de chaque paquet, et les scripts de
build Rust et les paquets source Python exécutent leur propre code d’installation. Craft Anything demande
confirmation avant une commande d’un type qu’il n’a pas encore vu dans ce projet (`npm`, `cargo`…).
Choisir « toujours autoriser » pour `npm` permet à toutes les commandes npm suivantes de s’exécuter sans
demander : ne le faites donc que pour un projet dont les dépendances vous inspirent confiance. Pour
installer des paquets npm sans leurs scripts d’installation, exécutez `npm install --ignore-scripts`.

## <a id="downloads"></a>Téléchargements

Sur GitHub, la [dernière release](https://github.com/storytold/craft-anything/releases/latest) contient chaque build listé ci-dessous, et [toutes les releases](https://github.com/storytold/craft-anything/releases) contiennent les versions antérieures et leurs notes. `<ver>` dans les noms de fichiers est le numéro de version, et `SHA256SUMS.txt` liste une somme de contrôle pour chaque fichier.

### <a id="windows"></a>Windows

| Build | Installateur | Portable |
|---|---|---|
| x64 (64-bit Intel/AMD) | `craft-anything-<ver>-windows-x64.msi` | `craft-anything-<ver>-windows-x64-portable.zip` |
| arm64 (Snapdragon et autres PC ARM) | `craft-anything-<ver>-windows-arm64.msi` | `craft-anything-<ver>-windows-arm64-portable.zip` |
| x86 (32-bit) | `craft-anything-<ver>-windows-x86.msi` | `craft-anything-<ver>-windows-x86-portable.zip` |

Les installateurs et exécutables sont signés numériquement. Le zip portable conserve ses paramètres à côté de l’application.

### <a id="macos"></a>macOS

| Build | Fichier | Notes |
|---|---|---|
| App, universal (Apple silicon + Intel) | `craft-anything-<ver>-macos-universal.dmg` | Signé et notarisé |
| Outil en ligne de commande, universal | `craft-anything-cli-<ver>-macos-universal.zip` | Signé et notarisé ; voir ci-dessous si macOS le bloque |

Les versions après 0.2.0 incluent l’outil en ligne de commande dans l’application, donc les builds n’ont pas besoin du zip séparé.
Vous n’en avez besoin que pour utiliser la ligne de commande sans l’application.

Un téléchargement du zip depuis le navigateur peut s’arrêter avec « Apple could not verify craft-anything-cli » (un programme isolé
ne peut pas porter sa notarisation avec lui, donc macOS doit le vérifier en ligne). Un téléchargement depuis
le terminal n’est pas vérifié de cette façon, donc utilisez plutôt ceci :

```sh
ver=0.2.0   # change this to the version you want
curl -fLO "https://github.com/storytold/craft-anything/releases/download/v$ver/craft-anything-cli-$ver-macos-universal.zip"
ditto -x -k "craft-anything-cli-$ver-macos-universal.zip" .
mkdir -p ~/.local/bin
mv "craft-anything-cli-$ver-macos-universal/craft-anything-cli" ~/.local/bin/
~/.local/bin/craft-anything-cli --version
```

Pour l’exécuter simplement comme `craft-anything-cli`, ajoutez `~/.local/bin` à votre `PATH`.

Si vous avez déjà la copie bloquée, appuyez sur Terminé (Done), pas sur Placer dans la corbeille (Move to Bin), puis exécutez
`xattr -d com.apple.quarantine <path to craft-anything-cli>`, ou autorisez-le dans Réglages Système →
Confidentialité et sécurité → Ouvrir quand même.

#### <a id="checking-that-a-mac-download-is-signed-and-notarized"></a>Vérifier qu’un téléchargement Mac est signé et notarisé

Chaque version Mac est signée avec un certificat Developer ID et notarisée par Apple. Pour vérifier
vous-même un téléchargement, exécutez ces commandes dans le Terminal (remplacez `<ver>` par la version et
les chemins par les vôtres) :

```sh
# The disk image: "accepted" and "source=Notarized Developer ID" mean it passed
spctl --assess --type open --context context:primary-signature -v "craft-anything-<ver>-macos-universal.dmg"
# The notarization ticket is attached: prints "The validate action worked!"
xcrun stapler validate "craft-anything-<ver>-macos-universal.dmg"

# The app, after you open the disk image: the signature must verify
codesign --verify --deep --strict --verbose=2 "/Volumes/<disk image name>/Craft Anything.app"
spctl --assess --type execute --verbose "/Volumes/<disk image name>/Craft Anything.app"

# The command-line tool: a bare program can't carry a ticket, so macOS checks it online
codesign --verify --strict --verbose=2 ~/.local/bin/craft-anything-cli
codesign -dv --verbose=4 ~/.local/bin/craft-anything-cli 2>&1 | grep Authority
```

Les lignes `Authority` doivent nommer un certificat « Developer ID Application ». Si une vérification
échoue, n’exécutez pas le fichier ; signalez-le en joignant la sortie et le tag de la version.

### <a id="linux"></a>Linux

| Format | x86_64 | aarch64 (ARM64) | Notes |
|---|---|---|---|
| AppImage | `craft-anything-<ver>-linux-x86_64.AppImage` | `craft-anything-<ver>-linux-aarch64.AppImage` | S’exécute partout ; se met à jour lui-même avec [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) (fichiers `.zsync`) |
| Flatpak | `craft-anything-<ver>-linux-x86_64.flatpak` | `craft-anything-<ver>-linux-aarch64.flatpak` | Sandboxé ; `flatpak install --user <file>` |
| Debian/Ubuntu | `craft-anything-<ver>-linux-x86_64.deb` | `craft-anything-<ver>-linux-aarch64.deb` | |
| Fedora/RHEL/openSUSE | `craft-anything-<ver>-linux-x86_64.rpm` | `craft-anything-<ver>-linux-aarch64.rpm` | |
| Tarball | `craft-anything-<ver>-linux-x86_64.tar.gz` | `craft-anything-<ver>-linux-aarch64.tar.gz` | Décompressez n’importe où |

RISC-V (riscv64) : `craft-anything-<ver>-linux-riscv64.tar.gz`, tarball uniquement.

L’AppImage contient aussi le CLI : exécutez `<AppImage> --cli <command>` (par exemple
`<AppImage> --cli mcp` pour un client MCP), ou créez un lien vers l’AppImage nommé
`craft-anything-cli` et exécutez-le.

### <a id="freebsd"></a>FreeBSD

| Build | Fichier |
|---|---|
| x86_64 | `craft-anything-<ver>-freebsd-x86_64.tar.gz` |

### <a id="web-webassembly"></a>Web (WebAssembly)

| Build | Fichier | Notes |
|---|---|---|
| Visionneuse navigateur | `craft-anything-web-<ver>.zip` | Dit ce qu’est un fichier, dans le navigateur (le fichier ne le quitte jamais) ; hébergez le dossier sur n’importe quel serveur web statique ([packaging/web/README.md](packaging/web/README.md)) |

<br>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Rejoignez la communauté ArtCraft sur Discord" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<h3 align="center"><a id="come-make-things-with-us"></a>Venez créer des choses avec nous</h3>

<p align="center">
  Notre Discord est l’endroit où les artistes de toutes sortes se retrouvent : des gens qui peignent, photographient, dessinent, montent des films,
  font de la typographie, et des gens qui cherchent encore ce qu’ils aiment créer. Partagez ce sur quoi vous travaillez,
  demandez de l’aide, dites-nous ce qui est cassé, ou dites-nous ce que vous aimeriez que ces outils puissent faire.
  Quel que soit votre médium et depuis combien de temps vous y êtes, vous êtes les bienvenus ici.
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><b>discord.gg/artcraft</b></a> ·
  <a href="https://getartcraft.com/">getartcraft.com</a> ·
  <a href="https://getartcraft.com/apps">Les applications Crafting</a> ·
  <a href="https://github.com/storytold/craft-anything">Craft Anything</a>
</p>


---

## <a id="the-craft-app-suite"></a>La suite d’applications Craft

| | App | À quoi ça sert | Code | En savoir plus |
|:-:|---|---|---|---|
| <img src="https://raw.githubusercontent.com/storytold/photocraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.photocraft.png" alt="" width="32" height="32"> | **PhotoCraft** | Édition d’images : calques, masques, typographie et vrais fichiers PSD | [GitHub](https://github.com/storytold/photocraft) | [Site web](https://getartcraft.com/apps/photocraft) |
| <img src="https://raw.githubusercontent.com/storytold/vectorcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.vectorcraft.png" alt="" width="32" height="32"> | **VectorCraft** | Illustration vectorielle | [GitHub](https://github.com/storytold/vectorcraft) | [Site web](https://getartcraft.com/apps/vectorcraft) |
| <img src="https://raw.githubusercontent.com/storytold/filmcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.filmcraft.png" alt="" width="32" height="32"> | **FilmCraft** | Montage vidéo, couleur et son | [GitHub](https://github.com/storytold/filmcraft) | [Site web](https://getartcraft.com/apps/filmcraft) |
| <img src="https://raw.githubusercontent.com/storytold/lightcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png" alt="" width="32" height="32"> | **LightCraft** | Bibliothèque photo et développement de fichiers RAW| [GitHub](https://github.com/storytold/lightcraft) | [Site web](https://getartcraft.com/apps/lightcraft) |
| <img src="https://raw.githubusercontent.com/storytold/pdfcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.pdfcraft.png" alt="" width="32" height="32"> | **PdfCraft** | Lecture, organisation et protection des PDF | [GitHub](https://github.com/storytold/pdfcraft) | [Site web](https://getartcraft.com/apps/pdfcraft) |
| <img src="https://raw.githubusercontent.com/storytold/effectcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png" alt="" width="32" height="32"> | **EffectCraft** | Animation graphique et effets visuels  | [GitHub](https://github.com/storytold/effectcraft) | [Site web](https://getartcraft.com/apps/effectcraft) |
| <img src="https://raw.githubusercontent.com/storytold/designcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.designcraft.png" alt="" width="32" height="32"> | **DesignCraft** | Mise en page et publication | [GitHub](https://github.com/storytold/designcraft) | [Site web](https://getartcraft.com/apps/designcraft) |
| <img src="https://raw.githubusercontent.com/storytold/figcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.figcraft.png" alt="" width="32" height="32"> | **FigCraft** | Conception d’interfaces et prototypage | [GitHub](https://github.com/storytold/figcraft) | [Site web](https://getartcraft.com/apps/figcraft) |
| <img src="https://raw.githubusercontent.com/storytold/wordcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.wordcraft.png" alt="" width="32" height="32"> | **WordCraft** | Traitement de texte et documents | [GitHub](https://github.com/storytold/wordcraft) | [Site web](https://getartcraft.com/apps/wordcraft) |
| <img src="https://raw.githubusercontent.com/storytold/soundcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.soundcraft.png" alt="" width="32" height="32"> | **SoundCraft** | Enregistrement et édition audio | [GitHub](https://github.com/storytold/soundcraft) | [Site web](https://getartcraft.com/apps/soundcraft) |
| <img src="https://raw.githubusercontent.com/storytold/deckcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.deckcraft.png" alt="" width="32" height="32"> | **DeckCraft** | Présentations et diaporamas | [GitHub](https://github.com/storytold/deckcraft) | [Site web](https://getartcraft.com/apps/deckcraft) |
| <img src="https://raw.githubusercontent.com/storytold/cadcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.cadcraft.png" alt="" width="32" height="32"> | **CADCraft** | Conception assistée par ordinateur | [GitHub](https://github.com/storytold/cadcraft) | [Site web](https://getartcraft.com/apps/cadcraft) |
| <img src="https://raw.githubusercontent.com/storytold/gridcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.gridcraft.png" alt="" width="32" height="32"> | **GridCraft** | Feuilles de calcul et données | [GitHub](https://github.com/storytold/gridcraft) | [Site web](https://getartcraft.com/apps/gridcraft) |
| <img src="https://raw.githubusercontent.com/storytold/texturecraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.texturecraft.png" alt="" width="32" height="32"> | **TextureCraft** | Peinture de textures 3D | [GitHub](https://github.com/storytold/texturecraft) | [Site web](https://getartcraft.com/apps/texturecraft) |
| <img src="https://raw.githubusercontent.com/storytold/materialcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.materialcraft.png" alt="" width="32" height="32"> | **MaterialCraft** | Matériaux procéduraux par nœuds | [GitHub](https://github.com/storytold/materialcraft) | [Site web](https://getartcraft.com/apps/materialcraft) |
| <img src="https://raw.githubusercontent.com/storytold/craft-anything/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.craft-anything.png" alt="" width="32" height="32"> | **Craft Anything** | Voir comment n’importe quel logiciel fonctionne, puis construire le vôtre | [GitHub](https://github.com/storytold/craft-anything) | [Site web](https://getartcraft.com/apps/craft-anything) |
| <img src="https://raw.githubusercontent.com/tangency-labs/solidcraft/main/assets/icons/app/linux/exports/hicolor/64x64/apps/io.tangencylabs.solidcraft.png" alt="" width="32" height="32"> | **SolidCraft** | CAO 3D paramétrique par Tangency Labs (application communautaire) | [GitHub](https://github.com/tangency-labs/solidcraft) | |


## <a id="license-and-credits"></a>Licence et crédits

Craft Anything est proposé sous double licence, [MIT](LICENSE-MIT) ou [Apache-2.0](LICENSE-APACHE), à votre choix.
Copyright (c) 2026 ArtCraft Team and the Craft Anything contributors. Les mentions requises se trouvent dans [NOTICE](NOTICE).

Les polices, icônes, images et autres assets groupés conservent leurs propres licences ouvertes ; chacun est listé
avec son auteur, sa source et sa licence dans [ATTRIBUTION.md](ATTRIBUTION.md).

Les captures d’écran montrent des projets d’exemple créés pour elles : example.com, un jeu de serpent et un petit
programme calculatrice.

Le nom ArtCraft, le wordmark et les logos dans [`docs/brand/`](docs/brand/) sont des marques de l’équipe
ArtCraft et ne sont pas couverts par cette licence. Ils ne peuvent être utilisés que non modifiés, et uniquement
dans le cadre de ce dépôt et de Craft Anything, selon [`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt).
Les forks et versions modifiées doivent les retirer.

<p align="center">
  <a href="https://getartcraft.com/"><img alt="ArtCraft" src="docs/brand/artcraft-mark.svg" width="28"></a><br>
  <sub>Fait par l’équipe <a href="https://getartcraft.com/">ArtCraft</a> et la communauté.</sub>
</p>

---

<sub>Cette page est la traduction française de [README.md](README.md). En cas de différence, la version anglaise fait foi.</sub>
