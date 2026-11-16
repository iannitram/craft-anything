<!-- Tradução do README.md para português brasileiro. Mantenha este arquivo alinhado à versão em inglês. -->

<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.craft-anything.png" width="128" height="128" alt="Ícone do Craft Anything: um corvo gravado em uma placa azul">
</p>

<h1 align="center">Craft Anything</h1>

<p align="center">
  <a href="README.md">English</a> · <a href="README.ja.md">日本語</a> · <a href="README.zh.md">简体中文</a> · <a href="README.zh-hant.md">繁體中文</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.fr.md">Français</a> · <a href="README.vi.md">Tiếng Việt</a> · <b>Português</b> · <a href="README.th.md">ไทย</a> · <a href="README.tr.md">Türkçe</a>
</p>

<p align="center">
  <b>Entenda como qualquer site, aplicativo ou jogo funciona e use IA para criar sua própria versão. Uma engine de código aberto em Rust puro, com abordagem clean room.</b><br>
  Indique o que você quer recriar e descreva o resultado desejado. O Craft Anything analisa o original com ferramentas reais de inspeção,<br>
  usa um agente de programação para criar um novo projeto, compara o resultado com o original e faz os refinamentos necessários.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux · FreeBSD · Web" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20FreeBSD%20%C2%B7%20Web-native-2d81ff?style=flat-square">
  <img alt="Licença: MIT OU Apache-2.0" src="https://img.shields.io/badge/licen%C3%A7a-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Estado do projeto: versão alpha inicial" src="https://img.shields.io/badge/fase-alpha%20inicial-d69e2e?style=flat-square">
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Participe da comunidade ArtCraft no Discord" src="https://img.shields.io/badge/Participe%20do%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="#downloads"><b>Baixar o Craft Anything</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">Todos os aplicativos</a>
</p>

<br>

<p align="center">
  <img src="docs/images/project-website.png" alt="Um projeto de site no Craft Anything: a verificação mostra o que ainda difere do original e a semelhança de cada página; o agente informa que sua versão está pronta, um cartão pede autorização antes de executar um comando e o plano mostra duas de quatro etapas concluídas" width="100%">
  <br>
  <sub>Um projeto de site: cada verificação mostra a semelhança com o original, você autoriza as ações que exigem sua decisão e o plano mostra o que falta.</sub>
</p>

> [!NOTE]
> **A ArtCraft é uma comunidade para quem cria, de todos os perfis e níveis de experiência.**
> Pintores, fotógrafos, cineastas, ilustradores, designers, animadores, quem cria por lazer
> e quem pegou em um lápis pela primeira vez na semana passada. Se você cria alguma coisa,
> já faz parte. **[Venha conversar no Discord](https://discord.gg/artcraft).**

<p align="center">
  <a href="#how-it-works">Como funciona</a> ·
  <a href="#two-ways-to-use-it">Dois modos de uso</a> ·
  <a href="#ai">IA</a> ·
  <a href="#built-for-agents">Agentes</a> ·
  <a href="#under-the-hood">Por dentro</a> ·
  <a href="#get-started">Primeiros passos</a> ·
  <a href="#status">Estado do projeto</a> ·
  <a href="#downloads">Downloads</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3>🔍 Explore qualquer software</h3>
      Sites, aplicativos para computador e celular, jogos e arquivos: ferramentas reais de inspeção identificam como foram feitos e o que fazem, com evidências para cada resposta.
    </td>
    <td width="25%" valign="top">
      <h3>🛠️ Crie sua própria versão</h3>
      Descreva o que você quer ("Recrie este site", "Recrie a jogabilidade deste jogo em Rust") e um agente de programação cria um novo projeto com a stack que você pedir.
    </td>
    <td width="25%" valign="top">
      <h3>✅ Resultados verificados</h3>
      Cada versão gerada é comparada com o original e refinada até ficar semelhante. A verificação inclui páginas em três larguras de tela, o autoteste do próprio jogo e execuções do aplicativo.
    </td>
    <td width="25%" valign="top">
      <h3>🤖 Pronto para agentes</h3>
      São 320 ferramentas, cada uma disponível como comando da CLI e ferramenta MCP. Assim, qualquer agente pode usar a mesma engine do aplicativo. A inspeção funciona sem IA.
    </td>
  </tr>
</table>

<br>

<a id="how-it-works"></a>

## Como funciona

Indique o que deseja analisar no Craft Anything, descreva o resultado que você quer e clique em **Iniciar**.
O trabalho segue quatro etapas. Quando uma decisão depende de você, o aplicativo faz a pergunta em linguagem simples:

1. **Estudar:** analisar o original — as páginas em diferentes larguras, o código, os arquivos e o comportamento durante a execução.
2. **Criar:** um agente de programação gera um novo projeto na linguagem, no framework ou na engine que você pedir, ou escolhe uma opção adequada quando você não especifica.
3. **Verificar:** comparar a versão criada com o original e listar as diferenças que ainda precisam de ajuste.
4. **Pronto:** abrir, jogar ou visualizar o resultado. Você também pode pedir alterações para continuar o refinamento.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/make.png" alt="Tela de criação: escolha um site, aplicativo ou jogo para recriar, informe o endereço, selecione Clean room ou Deep, descreva o que você quer e clique em Iniciar" width="100%"><br>
      <sub><b>Criar:</b> escolha o que recriar, descreva o que você quer e clique em Iniciar.</sub>
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/project-game.png" alt="Um projeto concluído do jogo da cobrinha, com todas as etapas finalizadas e o botão Jogar" width="100%"><br>
      <sub><b>Pronto:</b> jogue ou peça alterações no campo abaixo.</sub>
    </td>
  </tr>
</table>

<a id="two-ways-to-use-it"></a>

## Dois modos de uso

- **Simples** (padrão): a tela pergunta "O que você deseja criar?". Escolha um site, aplicativo ou jogo, descreva o que deseja e clique em **Iniciar**. A página do projeto mostra as quatro etapas (Estudar → Criar → Verificar → Pronto), explica o que o agente está fazendo, exibe cartões com perguntas e pedidos de autorização, informa a semelhança com o original e oferece botões para abrir o resultado ou a pasta. Peça alterações no campo da parte inferior. **Apenas examinar** abre um alvo para você fazer perguntas, sem gerar uma nova versão do software, no modo Deep.
- **Avançado:** a bancada completa, com todas as ferramentas (Ctrl/Cmd+K), funções, strings, pseudo-C, disassembly, visualização hexadecimal, grafos de chamadas, evidências, questões em aberto, tarefas, logs e o console do agente.

Os dois modos usam a mesma engine.

<a id="clean-room-or-deep"></a>

## Clean room ou Deep

- **Clean room** (padrão; **Sala limpa** na interface): não lê o código, os arquivos nem o código-fonte das páginas do original. O Craft Anything trabalha com o que é visível: imagens que você adiciona, o site como aparece a um visitante (capturas de tela, texto visível, medidas do layout, cores, fontes, espaçamentos e menus abertos) e, com sua autorização, uma captura da janela do aplicativo. As ferramentas que acessariam o conteúdo interno são bloqueadas e explicam como ativar o modo Deep.
- **Deep** (**Profundo** na interface): usa todas as ferramentas, para seu uso particular. O código e os arquivos de um programa são lidos pelas engines integradas e pelo Ghidra, quando instalado. O JavaScript do site é analisado e sua página inicial é observada durante a execução. O agente pode usar todo esse material para criar o projeto.

Clean room define o que o Craft Anything oferece e bloqueia. **Não é uma sandbox do sistema operacional:**
esse modo não isola o computador. O agente de programação continua atuando com o seu usuário do sistema,
e o que ele pode ler, gravar ou executar depende das permissões que possui. As ferramentas do agente que o Craft Anything
consegue bloquear variam conforme a integração. Para entender o tratamento das ferramentas do Claude Code
em conversas e perguntas, consulte [`docs/claude-code.md`](docs/claude-code.md).

Você pode alternar entre os modos na barra de título ou nas Configurações, usar
`settings_set deep_mode true` ou escolher `--deep` ou `--clean-room` para uma única execução pela CLI.

<p align="center">
  <img src="docs/images/workbench.png" alt="Bancada do modo Avançado: lista de funções, pseudo-C da função selecionada, o agente respondendo a uma pergunta com uma chamada de ferramenta e o registro de evidências" width="100%">
  <br>
  <sub><b>Avançado:</b> funções, pseudo-C, o agente e as evidências de cada resposta.</sub>
</p>

<a id="ai"></a>

## IA

Para criar projetos, é necessário ter um agente de programação instalado no computador.
A integração principal é com o **Claude Code**: o Craft Anything executa o seu próprio programa `claude`,
sem modificá-lo, e usa a sessão iniciada nele. O Craft Anything não acessa nem gerencia sua conta.
O uso continua sujeito ao seu próprio contrato com a Anthropic. Os detalhes e a justificativa estão em
[docs/claude-code.md](docs/claude-code.md).

Existe um adaptador para o Codex, mas ele ainda não foi validado em uma instalação real.
O **GitHub Copilot** pode usar a Copilot CLI instalada no computador por meio do ACP; faça login
separadamente com `copilot login`. Outros agentes compatíveis com ACP — Gemini CLI, Qwen Code,
opencode, goose, Hermes e Maki — podem ser configurados para usar esse protocolo em cada backend.
Entre eles, apenas o Maki foi testado em uma execução real.

Anthropic, OpenAI, OpenRouter e servidores locais compatíveis com a API da OpenAI estão disponíveis
para perguntas e análises, em **Configurações → IA**.

<a id="built-for-agents"></a>

## Feito para agentes

Cada ferramenta também está disponível como subcomando da CLI e ferramenta MCP. Assim, um agente
pode analisar um alvo ou executar uma recriação completa sem abrir o aplicativo:

```sh
craft-anything-cli commands                                   # lista as ferramentas por conjunto
craft-anything-cli mcp                                        # disponibiliza as ferramentas para clientes MCP
craft-anything-cli rebuild https://example.com "Recrie este site"
```

Os resultados são registros de evidências que informam o grau de certeza, a cobertura e as limitações.
Resultados grandes são apresentados primeiro como resumo; os detalhes podem ser consultados em páginas
com `core_evidence_browse`. O comando `craft-anything-cli setup` conecta o Craft Anything aos aplicativos
dos agentes de programação.

<a id="under-the-hood"></a>

## Por dentro do Craft Anything

Uma engine de engenharia reversa em Rust puro, com 320 ferramentas: 301 na engine e as demais nas camadas
de IA e configuração. Ela analisa programas nativos, .NET, Java, Android, Apple, Electron/JavaScript,
sites, firmware, capturas de pacotes, bancos de dados e arquivos de jogos.

Os decodificadores, o disassembler e o descompilador integrados funcionam sem programas externos.
O modo Deep usa todos eles e também o Ghidra, quando instalado. Essa engine é a base do aplicativo
desktop (egui), da CLI e do servidor MCP. Parte dela também funciona no navegador, via WebAssembly.
A lista completa de ferramentas está em [docs/tools.md](docs/tools.md).

<a id="get-started"></a>

## Primeiros passos

É necessário ter **Rust 1.97 ou mais recente**, por exigência de uma dependência. Atualize com
`rustup update`. No Debian e no Ubuntu, o aplicativo desktop também precisa destes pacotes:

```sh
sudo apt-get install build-essential libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev libgtk-3-dev
```

Depois:

```sh
git clone https://github.com/storytold/craft-anything && cd craft-anything
cargo run --release -p craft-anything                # inicia o aplicativo desktop
cargo install --path apps/craft-anything-cli         # instala a CLI como craft-anything-cli
craft-anything-cli doctor                            # verifica a instalação, os agentes e as engines
```

Para criar projetos, o agente de programação precisa estar autenticado no computador (consulte
[IA](#ai)). O comando `doctor` informa o que falta. Exemplos de recriação pela linha de comando,
com as perguntas respondidas no terminal:

```sh
craft-anything-cli rebuild https://example.com "Recrie este site"
craft-anything-cli rebuild - "Um jogo da cobrinha em Rust" --rounds 1
craft-anything-cli rebuild https://example.com "Deixe o cabeçalho azul" --continue-in "<pasta do projeto>"
craft-anything-cli rebuild - "Adicione um formulário de contato" --project-folder "<sua pasta>" --existing edit
```

Por padrão, os novos projetos ficam em `Documents/Craft Anything`. Você pode mudar esse local em
**Configurações → Geral**. Cada pasta de projeto contém:

- `craft.json`: solicitação, alvo, estado e histórico do projeto;
- `capture/`: material coletado do original;
- `app/`: projeto gerado;
- `checks/`: comparações com o original;
- `investigation.cany/`: evidências da investigação;
- `inbox/`: perguntas pendentes.

Você também pode iniciar um projeto em uma pasta de sua escolha: use **Escolher uma pasta…**, abaixo
de **Iniciar**, ou a opção `--project-folder`. Se a pasta já tiver arquivos, informe se eles são o
próprio aplicativo, que será editado no local, ou materiais de referência para criar o aplicativo
em `app/`, ao lado deles. **Configurações → Geral** pode salvar essa preferência. Essa tela também
permite listar projetos de outras pastas. Remover uma entrada apenas a retira da lista, sem excluir o projeto.

<a id="status"></a>

## Estado do projeto

> [!IMPORTANT]
> O Craft Anything está em uma **fase alpha inicial**. Veja o que já funciona, o que ainda está incompleto
> e os próximos passos:
>
> - **Já funciona:** o aplicativo desktop nos modos Simples e Avançado, a CLI e o servidor MCP; o fluxo
>   Estudar → Criar → Verificar → Refinar para sites, aplicativos e jogos com o Claude Code como agente
>   de programação; comparação de sites em três larguras de tela; recriação de jogos em Unity, Unreal
>   ou Godot; modo Deep com Ghidra; geração opcional de imagens e modelos 3D com sua própria chave
>   do fal.ai; plugins como servidores MCP e, nas versões compiladas com a opção `wasm-plugins`, componentes WASM.
> - **Ainda está incompleto:** criação de projetos com chaves de API e outros agentes de programação
>   (implementada, mas ainda sem validação em uso real), verificações de comportamento de aplicativos
>   e jogos e algumas ferramentas da engine testadas apenas com componentes simulados.
> - **Versões publicadas:** há releases com instaladores para Windows, macOS, Linux e FreeBSD na
>   [página de releases](https://github.com/storytold/craft-anything/releases). Elas continuam sendo versões alpha.
> - **Próximo passo:** uma modalidade de serviço gerenciado.
>
> [docs/STATUS.md](docs/STATUS.md) detalha o funcionamento e as verificações realizadas.
> [docs/tool-status.md](docs/tool-status.md) traz a situação de cada ferramenta, e
> [docs/ROADMAP.md](docs/ROADMAP.md) apresenta o plano de desenvolvimento, item por item.
> Abra uma issue ou conte o que deu errado no [Discord](https://discord.gg/artcraft).

**Uso responsável.** Use o Craft Anything em softwares que sejam seus ou que você tenha autorização
para examinar. Ele não contorna DRM, logins nem anti-cheat. A recriação reproduz layout, estilo e
comportamento com base em evidências. O agente é orientado a não copiar logotipos, fotos ou textos,
exceto quando você confirma que é o titular ou tem permissão para usá-los.

**Instalar dependências executa código.** Quando o agente instala as dependências de um projeto,
os comandos rodam no seu computador, e os pacotes podem executar código durante a instalação.
Por padrão, o npm executa os scripts de instalação de cada pacote; os scripts de compilação do Rust e os
pacotes distribuídos como código-fonte em Python também executam sua própria configuração.
O Craft Anything pede autorização para um tipo de comando que ainda não tenha sido usado naquele
projeto (`npm`, `cargo`…). Autorizar permanentemente o `npm` permite que todos os comandos
npm seguintes sejam executados sem uma nova pergunta. Faça essa escolha apenas em projetos cujas
dependências você considera confiáveis. Para instalar pacotes npm sem executar os scripts de
instalação, use `npm install --ignore-scripts`.

## Downloads

A [versão mais recente](https://github.com/storytold/craft-anything/releases/latest) no GitHub reúne
todas as builds abaixo. A [lista de releases](https://github.com/storytold/craft-anything/releases)
contém as versões anteriores e suas notas. Nos nomes dos arquivos, `<ver>` representa a versão.
O arquivo `SHA256SUMS.txt` lista o hash SHA-256 de cada arquivo.

### Windows

| Arquitetura | Instalador | Versão portátil |
|---|---|---|
| x64 (Intel/AMD de 64 bits) | `craft-anything-<ver>-windows-x64.msi` | `craft-anything-<ver>-windows-x64-portable.zip` |
| arm64 (Snapdragon e outros PCs ARM) | `craft-anything-<ver>-windows-arm64.msi` | `craft-anything-<ver>-windows-arm64-portable.zip` |
| x86 (32 bits) | `craft-anything-<ver>-windows-x86.msi` | `craft-anything-<ver>-windows-x86-portable.zip` |

Os instaladores e executáveis são assinados digitalmente. O ZIP portátil mantém as configurações
na mesma pasta do aplicativo.

### macOS

| Versão | Arquivo | Observações |
|---|---|---|
| Aplicativo universal (Apple silicon + Intel) | `craft-anything-<ver>-macos-universal.dmg` | Assinado digitalmente e notarizado pela Apple |
| Ferramenta de linha de comando universal | `craft-anything-cli-<ver>-macos-universal.zip` | Assinada digitalmente e notarizada; veja abaixo o que fazer se o macOS a bloquear |

As versões posteriores à 0.2.0 já incluem a CLI no aplicativo. Para criar projetos, não é necessário
baixar o ZIP separado. Ele só é necessário para usar a linha de comando sem instalar o aplicativo.

Ao baixar o ZIP pelo navegador, o macOS pode exibir a mensagem "Apple could not verify craft-anything-cli".
Um executável avulso não permite anexar o ticket de notarização; o macOS precisa
consultá-lo pela internet. Um download feito pelo terminal não passa por essa mesma verificação.
Nesse caso, use:

```sh
ver=0.2.0   # substitua pela versão desejada
curl -fLO "https://github.com/storytold/craft-anything/releases/download/v$ver/craft-anything-cli-$ver-macos-universal.zip"
ditto -x -k "craft-anything-cli-$ver-macos-universal.zip" .
mkdir -p ~/.local/bin
mv "craft-anything-cli-$ver-macos-universal/craft-anything-cli" ~/.local/bin/
~/.local/bin/craft-anything-cli --version
```

Para executar apenas `craft-anything-cli`, adicione `~/.local/bin` ao seu `PATH`.

Se você já baixou uma cópia bloqueada, clique em **Done** (Concluído), e não em **Move to Bin**
(Mover para a Lixeira). Depois, execute `xattr -d com.apple.quarantine <caminho do craft-anything-cli>`
ou autorize a abertura em **Ajustes do Sistema → Privacidade e Segurança → Abrir Mesmo Assim**.

#### Conferir a assinatura e a notarização de um download no Mac

Cada release para Mac é assinada com um certificado Developer ID e notarizada pela Apple.
Para conferir um download, execute os comandos abaixo no Terminal, substituindo `<ver>` pela
versão e ajustando os caminhos:

```sh
# Imagem de disco: "accepted" e "source=Notarized Developer ID" indicam aprovação
spctl --assess --type open --context context:primary-signature -v "craft-anything-<ver>-macos-universal.dmg"
# O ticket de notarização está anexado: a saída esperada é "The validate action worked!"
xcrun stapler validate "craft-anything-<ver>-macos-universal.dmg"

# Aplicativo, depois de abrir a imagem de disco: a assinatura deve ser válida
codesign --verify --deep --strict --verbose=2 "/Volumes/<nome da imagem de disco>/Craft Anything.app"
spctl --assess --type execute --verbose "/Volumes/<nome da imagem de disco>/Craft Anything.app"

# CLI: o executável avulso não inclui o ticket; o macOS o consulta pela internet
codesign --verify --strict --verbose=2 ~/.local/bin/craft-anything-cli
codesign -dv --verbose=4 ~/.local/bin/craft-anything-cli 2>&1 | grep Authority
```

As linhas `Authority` devem indicar um certificado "Developer ID Application". Se alguma
verificação falhar, não execute o arquivo: relate o problema com a saída do comando e a tag do release.

### Linux

| Formato | x86_64 | aarch64 (ARM64) | Observações |
|---|---|---|---|
| AppImage | `craft-anything-<ver>-linux-x86_64.AppImage` | `craft-anything-<ver>-linux-aarch64.AppImage` | Execução portátil; atualização automática com [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) e arquivos `.zsync` |
| Flatpak | `craft-anything-<ver>-linux-x86_64.flatpak` | `craft-anything-<ver>-linux-aarch64.flatpak` | Em sandbox; `flatpak install --user <arquivo>` |
| Debian/Ubuntu | `craft-anything-<ver>-linux-x86_64.deb` | `craft-anything-<ver>-linux-aarch64.deb` | |
| Fedora/RHEL/openSUSE | `craft-anything-<ver>-linux-x86_64.rpm` | `craft-anything-<ver>-linux-aarch64.rpm` | |
| Tarball | `craft-anything-<ver>-linux-x86_64.tar.gz` | `craft-anything-<ver>-linux-aarch64.tar.gz` | Extraia na pasta de sua escolha |

Para RISC-V (`riscv64`), há apenas o tarball `craft-anything-<ver>-linux-riscv64.tar.gz`.

A AppImage também inclui a CLI. Execute `<AppImage> --cli <comando>` — por exemplo,
`<AppImage> --cli mcp` para um cliente MCP — ou crie um link para a AppImage chamado
`craft-anything-cli` e execute esse link.

### FreeBSD

| Arquitetura | Arquivo |
|---|---|
| x86_64 | `craft-anything-<ver>-freebsd-x86_64.tar.gz` |

### Web (WebAssembly)

| Versão | Arquivo | Observações |
|---|---|---|
| Visualizador no navegador | `craft-anything-web-<ver>.zip` | Identifica o tipo de arquivo no navegador, sem enviá-lo para um servidor; hospede a pasta em qualquer servidor de arquivos estáticos ([packaging/web/README.md](packaging/web/README.md)) |

<br>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Participe da comunidade ArtCraft no Discord" src="https://img.shields.io/badge/Participe%20do%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<h3 align="center">Venha criar com a gente</h3>

<p align="center">
  Nosso Discord reúne pessoas das mais diversas áreas: pintura, fotografia, desenho, cinema, tipografia e quem ainda está descobrindo o que gosta de criar.
  Compartilhe seus projetos, peça ajuda, conte o que deu errado ou diga o que gostaria de fazer com estas ferramentas.
  Seja qual for sua área ou seu nível de experiência, você é bem-vindo.
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><b>discord.gg/artcraft</b></a> ·
  <a href="https://getartcraft.com/">getartcraft.com</a> ·
  <a href="https://getartcraft.com/apps">Crafting Apps</a> ·
  <a href="https://github.com/storytold/craft-anything">Craft Anything</a>
</p>

---

<a id="the-craft-app-suite"></a>

## Suíte de aplicativos Craft

| | Aplicativo | Finalidade | Código | Saiba mais |
|:-:|---|---|---|---|
| <img src="https://raw.githubusercontent.com/storytold/photocraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.photocraft.png" alt="" width="32" height="32"> | **PhotoCraft** | Edição de imagens: camadas, máscaras, texto e arquivos PSD nativos | [GitHub](https://github.com/storytold/photocraft) | [Site](https://getartcraft.com/apps/photocraft) |
| <img src="https://raw.githubusercontent.com/storytold/vectorcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.vectorcraft.png" alt="" width="32" height="32"> | **VectorCraft** | Ilustração vetorial | [GitHub](https://github.com/storytold/vectorcraft) | [Site](https://getartcraft.com/apps/vectorcraft) |
| <img src="https://raw.githubusercontent.com/storytold/filmcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.filmcraft.png" alt="" width="32" height="32"> | **FilmCraft** | Edição de vídeo, cor e som | [GitHub](https://github.com/storytold/filmcraft) | [Site](https://getartcraft.com/apps/filmcraft) |
| <img src="https://raw.githubusercontent.com/storytold/lightcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png" alt="" width="32" height="32"> | **LightCraft** | Biblioteca de fotos e revelação de arquivos RAW | [GitHub](https://github.com/storytold/lightcraft) | [Site](https://getartcraft.com/apps/lightcraft) |
| <img src="https://raw.githubusercontent.com/storytold/pdfcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.pdfcraft.png" alt="" width="32" height="32"> | **PdfCraft** | Leitura, organização e proteção de PDFs | [GitHub](https://github.com/storytold/pdfcraft) | [Site](https://getartcraft.com/apps/pdfcraft) |
| <img src="https://raw.githubusercontent.com/storytold/effectcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png" alt="" width="32" height="32"> | **EffectCraft** | Motion graphics e efeitos visuais | [GitHub](https://github.com/storytold/effectcraft) | [Site](https://getartcraft.com/apps/effectcraft) |
| <img src="https://raw.githubusercontent.com/storytold/designcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.designcraft.png" alt="" width="32" height="32"> | **DesignCraft** | Diagramação e publicação | [GitHub](https://github.com/storytold/designcraft) | [Site](https://getartcraft.com/apps/designcraft) |
| <img src="https://raw.githubusercontent.com/storytold/figcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.figcraft.png" alt="" width="32" height="32"> | **FigCraft** | Design de interfaces e prototipagem | [GitHub](https://github.com/storytold/figcraft) | [Site](https://getartcraft.com/apps/figcraft) |
| <img src="https://raw.githubusercontent.com/storytold/wordcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.wordcraft.png" alt="" width="32" height="32"> | **WordCraft** | Edição de textos e documentos | [GitHub](https://github.com/storytold/wordcraft) | [Site](https://getartcraft.com/apps/wordcraft) |
| <img src="https://raw.githubusercontent.com/storytold/soundcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.soundcraft.png" alt="" width="32" height="32"> | **SoundCraft** | Gravação, edição e mixagem de áudio | [GitHub](https://github.com/storytold/soundcraft) | [Site](https://getartcraft.com/apps/soundcraft) |
| <img src="https://raw.githubusercontent.com/storytold/deckcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.deckcraft.png" alt="" width="32" height="32"> | **DeckCraft** | Criação de apresentações | [GitHub](https://github.com/storytold/deckcraft) | [Site](https://getartcraft.com/apps/deckcraft) |
| <img src="https://raw.githubusercontent.com/storytold/cadcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.cadcraft.png" alt="" width="32" height="32"> | **CADCraft** | Projeto assistido por computador (CAD) | [GitHub](https://github.com/storytold/cadcraft) | [Site](https://getartcraft.com/apps/cadcraft) |
| <img src="https://raw.githubusercontent.com/storytold/gridcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.gridcraft.png" alt="" width="32" height="32"> | **GridCraft** | Planilhas e dados | [GitHub](https://github.com/storytold/gridcraft) | [Site](https://getartcraft.com/apps/gridcraft) |
| <img src="https://raw.githubusercontent.com/storytold/texturecraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.texturecraft.png" alt="" width="32" height="32"> | **TextureCraft** | Pintura de texturas 3D | [GitHub](https://github.com/storytold/texturecraft) | [Site](https://getartcraft.com/apps/texturecraft) |
| <img src="https://raw.githubusercontent.com/storytold/materialcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.materialcraft.png" alt="" width="32" height="32"> | **MaterialCraft** | Materiais procedurais por nós | [GitHub](https://github.com/storytold/materialcraft) | [Site](https://getartcraft.com/apps/materialcraft) |
| <img src="https://raw.githubusercontent.com/storytold/craft-anything/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.craft-anything.png" alt="" width="32" height="32"> | **Craft Anything** | Entenda como um software funciona e crie sua própria versão | [GitHub](https://github.com/storytold/craft-anything) | [Site](https://getartcraft.com/apps/craft-anything) |
| <img src="https://raw.githubusercontent.com/tangency-labs/solidcraft/main/assets/icons/app/linux/exports/hicolor/64x64/apps/io.tangencylabs.solidcraft.png" alt="" width="32" height="32"> | **SolidCraft** | CAD 3D paramétrico da Tangency Labs (aplicativo da comunidade) | [GitHub](https://github.com/tangency-labs/solidcraft) | |

<a id="license-and-credits"></a>

## Licença e créditos

O Craft Anything é disponibilizado sob duas opções de licença: [MIT](LICENSE-MIT) ou
[Apache-2.0](LICENSE-APACHE), à sua escolha.
Copyright (c) 2026 ArtCraft Team e colaboradores do Craft Anything. Os avisos obrigatórios estão
em [NOTICE](NOTICE).

Fontes, ícones, imagens e outros recursos incluídos mantêm suas respectivas licenças abertas.
O autor, a origem e a licença de cada um estão registrados em [ATTRIBUTION.md](ATTRIBUTION.md).

As capturas de tela mostram projetos de exemplo criados para elas: example.com, um jogo da cobrinha
e um pequeno programa de calculadora.

O nome ArtCraft, o wordmark (logotipo tipográfico) e os logotipos em [`docs/brand/`](docs/brand/) são marcas comerciais da
equipe ArtCraft e não são abrangidos por esta licença. Seu uso é permitido apenas sem modificações
e como parte deste repositório e do Craft Anything, nos termos de
[`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt). Forks e versões modificadas devem removê-los.

<p align="center">
  <a href="https://getartcraft.com/"><img alt="ArtCraft" src="docs/brand/artcraft-mark.svg" width="28"></a><br>
  <sub>Feito pela equipe <a href="https://getartcraft.com/">ArtCraft</a> e pela comunidade.</sub>
</p>
