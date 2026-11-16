<p align="center">
  <a href="https://getartcraft.com/">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="docs/brand/artcraft-logo-white.svg">
      <img alt="ArtCraft" src="docs/brand/artcraft-logo.svg" width="200">
    </picture>
  </a>
</p>

<p align="center">
  <img src="assets/app-icon/hicolor/256x256/apps/ai.storyteller.craft-anything.png" width="128" height="128" alt="Craft Anything simgesi: mavi bir karo üzerinde kazınmış bir kuzgun">
</p>

<h1 align="center">Craft Anything</h1>

<p align="center">
  <a href="README.md">English</a> · <a href="README.ja.md">日本語</a> · <a href="README.zh.md">简体中文</a> · <a href="README.zh-hant.md">繁體中文</a> · <a href="README.ko.md">한국어</a> · <a href="README.es.md">Español</a> · <a href="README.fr.md">Français</a> · <a href="README.vi.md">Tiếng Việt</a> · <a href="README.pt.md">Português</a> · <a href="README.th.md">ไทย</a> · <b>Türkçe</b>
</p>

<p align="center">
  <b>Herhangi bir web sitesinin, uygulamanın veya oyunun nasıl çalıştığını gör, sonra yapay zekâya kendi sürümünü yaptır; saf Rust ile yazılmış, açık kaynaklı, clean-room bir motor.</b><br>
  Bir hedef göster, ne istediğini söyle; orijinali gerçek inceleme araçlarıyla çalışır,<br>
  bir kodlama ajanına yeni bir proje yaptırır, sonucu orijinalle karşılaştırır ve iyileştirir.
</p>

<p align="center">
  <img alt="100% Rust" src="https://img.shields.io/badge/100%25-Rust-b7410e?style=flat-square&logo=rust">
  <img alt="macOS · Windows · Linux · FreeBSD · Web" src="https://img.shields.io/badge/macOS%20%C2%B7%20Windows%20%C2%B7%20Linux%20%C2%B7%20FreeBSD%20%C2%B7%20Web-native-2d81ff?style=flat-square">
  <img alt="Lisans: MIT VEYA Apache-2.0" src="https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square">
  <img alt="Durum: erken alfa" src="https://img.shields.io/badge/status-early%20alpha-d69e2e?style=flat-square">
  <br>
  <a href="https://sonarcloud.io/project/overview?id=storytold_craft-anything"><img alt="SonarCloud ile tarandı" src="https://img.shields.io/badge/Scanned_by-SonarCloud-4b9fd5?style=flat-square&logo=sonarqubecloud&logoColor=white"></a>
  <a href="https://coderabbit.ai"><img alt="CodeRabbit ile incelendi" src="https://img.shields.io/coderabbit/prs/github/storytold/craft-anything?style=flat-square&label=Reviewed%20by%20CodeRabbit&logo=coderabbit"></a>
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Discord'da ArtCraft topluluğuna katıl" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<p align="center">
  <a href="#indirmeler"><b>Craft Anything'i indir</b></a> ·
  <a href="https://getartcraft.com/">ArtCraft</a> ·
  <a href="https://getartcraft.com/apps">Tüm Craft uygulamaları</a>
</p>

<br>

<p align="center">
  <img src="docs/images/project-website.png" alt="Craft Anything'de bir web sitesi projesi: kontrol, henüz neyin eşleşmediğini ve her sayfanın ne kadar benzediğini listeliyor; yapıcı kendi sürümünün hazır olduğunu söylüyor; bir onay kartı komut çalıştırmadan önce izin istiyor ve plan dört adımın ikisinin bittiğini gösteriyor" width="100%">
  <br>
  <sub>Bir web sitesi projesi: her kontrol, yapılan sürümün ne kadar benzediğini söyler; kuralların dışındaki her şeyi sen onaylarsın ve plan geriye ne kaldığını gösterir.</sub>
</p>

> [!NOTE]
> **ArtCraft, hayatın her kesiminden sanatçıların oluşturduğu bir topluluktur.** Ressamlar, fotoğrafçılar,
> film yapımcıları, illüstratörler, tasarımcılar, animatörler, hobiciler ve geçen hafta eline kalem alanlar.
> Bir şeyler üretiyorsan, sen de bizdensin. **[Discord'a gel, merhaba de](https://discord.gg/artcraft).**

<p align="center">
  <a href="#nasıl-çalışır">Nasıl çalışır</a> ·
  <a href="#iki-kullanim">İki kullanım şekli</a> ·
  <a href="#yapay-zekâ">Yapay zekâ</a> ·
  <a href="#ajanlar-için-yapıldı">Ajanlar</a> ·
  <a href="#perde-arkası">Perde arkası</a> ·
  <a href="#başlarken">Başlarken</a> ·
  <a href="#durum">Durum</a> ·
  <a href="#indirmeler">İndirmeler</a> ·
  <a href="https://discord.gg/artcraft">Discord</a>
</p>

<br>

<table>
  <tr>
    <td width="25%" valign="top">
      <h3>🔍 Her şeyi incele</h3>
      Web siteleri, masaüstü uygulamaları, telefon uygulamaları, oyunlar ve dosyalar: gerçek inceleme araçları nasıl yapıldıklarını ve ne yaptıklarını bulur, her cevabın arkasında kanıtıyla.
    </td>
    <td width="25%" valign="top">
      <h3>🛠️ Kendininkini yap</h3>
      Ne istediğini söyle ("Bu web sitesini yeniden yap", "Bu oyunun oynanışını Rust ile yeniden yap"), bir kodlama ajanı istediğin teknolojiyle yeni bir proje kursun.
    </td>
    <td width="25%" valign="top">
      <h3>✅ Tahmin değil, kontrol</h3>
      Her sürüm orijinalle karşılaştırılır (üç ekran genişliğinde sayfalar, oyunun kendi öz testi, uygulamanın çalıştırmaları) ve yakın olana kadar iyileştirilir.
    </td>
    <td width="25%" valign="top">
      <h3>🤖 Ajanlara hazır</h3>
      320 araç; her biri bir CLI komutu ve bir MCP aracı. Böylece her ajan uygulamanın kullandığı motoru kullanabilir. İnceleme için yapay zekâ gerekmez.
    </td>
  </tr>
</table>

<br>

## Nasıl çalışır

Craft Anything'e bir hedef göster, ne istediğini söyle ve Start'a bas. Dört adımda çalışır;
bir karar sana kaldığında sade bir dille sorar:

1. **İncele (Study):** orijinali inceler: her genişlikteki sayfalarını, kodunu, dosyalarını, nasıl çalıştığını.
2. **Yap (Build):** bir kodlama ajanı, isteğinde belirttiğin dil, framework veya oyun motoruyla
   (ya da mantıklı bir varsayılanla) yeni bir proje yapar.
3. **Kontrol et (Check):** yapılan sürümü orijinalle karşılaştırır ve henüz neyin eşleşmediğini listeler.
4. **Hazır (Ready):** sonucu aç, oyna ya da önizle; veya değişiklik iste, o da iyileştirsin.

<table>
  <tr>
    <td width="50%" valign="top">
      <img src="docs/images/make.png" alt="Make ekranı: yeniden yapılacak bir web sitesi, uygulama veya oyun seç, adresi yaz, Clean room ya da Deep seç, ne istediğini söyle ve Start'a bas" width="100%"><br>
      <sub><b>Make:</b> neyi yeniden yapacağını seç, ne istediğini söyle, Start'a bas.</sub>
    </td>
    <td width="50%" valign="top">
      <img src="docs/images/project-game.png" alt="Tüm adımları bitmiş, Play the game butonu olan tamamlanmış bir yılan oyunu projesi" width="100%"><br>
      <sub><b>Ready:</b> oyunu oyna ya da aşağıdan değişiklik iste.</sub>
    </td>
  </tr>
</table>

<a id="iki-kullanim"></a>

## İki kullanım şekli

- **Basit** (varsayılan): "Ne yapmak istiyorsun?" Bir web sitesi, uygulama veya oyun seç, tarif et,
  Start'a bas. Proje sayfası dört adımı (Study → Build → Check → Ready), ajanın ne yaptığını sade bir dille,
  soru ve onay kartlarını, sonucun ne kadar benzediğini ve sonucu ya da klasörünü açan butonları gösterir.
  Değişiklikleri alttaki kutudan iste. "Just look inside" hiçbir şey yapmadan bir hedefi sorular için açar (Deep modunda).
- **Gelişmiş**: tam çalışma tezgâhı: her araç (Ctrl/Cmd+K), fonksiyonlar, metinler (strings), sözde C (pseudo-C),
  disassembly, hex, çağrı grafikleri, kanıtlar, bilinmeyenler, işler, loglar ve ajan konsolu.

İki mod da aynı motoru kullanır.

## Clean room ya da Deep

- **Clean room** (varsayılan): orijinalin içindeki hiçbir şey okunmaz; ne kodu, ne dosyaları, ne de sayfa
  kaynağı. Craft Anything görebildiklerinden çalışır: eklediğin resimler, bir ziyaretçinin gördüğü haliyle web sitesi
  (ekran görüntüleri, görünür metin, ölçülmüş yerleşim, renkler, yazı tipleri ve boşluklar, açılan menüler)
  ve izin verirsen uygulama penceresinin bir resmi. İçeriye bakacak araçlar reddedilir
  ve Deep modunun nasıl açılacağını söyler.
- **Deep**: her araç kullanılır, kişisel kullanımın için. Bir programın kodu ve dosyaları yerleşik
  motorlarla (ve kuruluysa Ghidra ile) okunur, bir web sitesinin JavaScript'i okunur ve başlangıç
  sayfası izlenir; yapıcı bunların hepsini kullanabilir.

Clean room, Craft Anything'in neyi sunup neyi reddettiğine dair bir kuraldır. Bir işletim sistemi
sandbox'ı değildir: bilgisayarını izole etmez. Çalıştırdığın bir kodlama ajanı yine senin adına hareket eder; neyi
okuyabileceği, yazabileceği veya çalıştırabileceği kendi izinlerine bağlıdır. Craft Anything'in bir ajanın kendi araçlarından
hangilerini reddedebileceği ajana göre değişir; Claude Code'un sohbet ve sorulardaki araçları için
[`docs/claude-code.md`](docs/claude-code.md) belgesine bak.

Başlık çubuğundan veya Ayarlar'dan, `settings_set deep_mode true` ile ya da tek bir CLI çalıştırması için
`--deep` (veya `--clean-room`) ile geçiş yap.

<p align="center">
  <img src="docs/images/workbench.png" alt="Gelişmiş çalışma tezgâhı: bir fonksiyon listesi, seçili fonksiyonun sözde C kodu, bir soruyu araç çağrısıyla yanıtlayan ajan ve kanıt defteri" width="100%">
  <br>
  <sub><b>Gelişmiş:</b> fonksiyonlar, sözde C, ajan ve her cevabın arkasındaki kanıt.</sub>
</p>

## Yapay zekâ

Yapım için bilgisayarında bir kodlama ajanı gerekir. İlk desteklenen **Claude Code**'dur: Craft
Anything senin kendi, değiştirilmemiş `claude` programını çalıştırır; o da senin kendi oturumunu kullanır. Craft Anything
hesabını asla görmez ya da işlemez. Kullanımın, Anthropic ile kendi anlaşmana tabidir.
Nasıl ve neden olduğu için [docs/claude-code.md](docs/claude-code.md) belgesine bak. Bir Codex adaptörü
var ama henüz gerçek bir kurulumda doğrulanmadı. **GitHub Copilot**, kurulu Copilot CLI'ını ACP üzerinden
kullanabilir: `copilot login` ile ayrıca giriş yap. **Grok Build**, kurulu `grok` programını ACP üzerinden
(`grok agent stdio`) kullanabilir: `grok login` ile ayrıca giriş yap. [docs/grok.md](docs/grok.md) belgesine bak.
ACP konuşan diğer ajanlar (Gemini CLI, Qwen Code, opencode, goose, Hermes, Maki, Devin) her arka uç için
buna geçirilebilir; bunlardan Maki ve Devin canlı olarak çalıştırıldı (Devin, arka ucun kayıtlı anahtarıyla
kimlik doğrular; kendi CLI girişi ACP üzerinden geçerli sayılmaz). Anthropic, OpenAI, OpenRouter, xAI (`XAI_API_KEY`
ayarlıysa Grok) ve yerel OpenAI uyumlu sunucular sorular ve analiz için kullanılabilir (Ayarlar → AI).

## Ajanlar için yapıldı

Her araç aynı zamanda bir CLI alt komutu ve bir MCP aracıdır; böylece bir ajan, uygulama olmadan
bir hedefi inceleyebilir ya da baştan sona bir yeniden yapım çalıştırabilir:

```sh
craft-anything-cli commands                                   # araç setine göre her araç
craft-anything-cli mcp                                        # hepsini herhangi bir MCP istemcisine sun
craft-anything-cli rebuild https://example.com "Recreate this website"
```

Sonuçlar; güven düzeyi, kapsamı ve sınırlamalarıyla birlikte kanıt kayıtlarıdır. Büyük olanlar önce özet olarak
gelir ve `core_evidence_browse` ile sayfa sayfa gezilir. `craft-anything-cli setup`,
Craft Anything'i kodlama ajanı uygulamalarına bağlar.

## Perde arkası

Yerel programlar, .NET, Java, Android, Apple, Electron/JavaScript, web siteleri, firmware,
paket yakalamaları, veritabanları ve oyun dosyaları için 320 araçlı (300'ü motorda, kalanı yapay zekâ ve
kurulum katmanlarında) saf Rust bir tersine mühendislik motoru. Kendi çözücüleri (decoder), disassembler'ı ve
decompiler'ı hiçbir dış programa ihtiyaç duymadan çalışır; Deep modu hepsini ve kuruluysa Ghidra'yı kullanır.
Masaüstü uygulamasına (egui), CLI'a ve MCP sunucusuna güç verir; parçaları tarayıcıda WebAssembly olarak çalışır.
Tam liste [docs/tools.md](docs/tools.md) belgesinde.

## Başlarken

Rust **1.97 veya üzeri** gerekir (bir bağımlılık bunu istiyor; `rustup update`). Debian veya Ubuntu'da
masaüstü uygulaması için ayrıca şunlar gerekir:

```sh
sudo apt-get install build-essential libxkbcommon-dev libxkbcommon-x11-0 libwayland-dev libx11-dev libxrandr-dev libxi-dev libgl1-mesa-dev libgtk-3-dev
```

Sonra:

```sh
git clone https://github.com/storytold/craft-anything && cd craft-anything
cargo run --release -p craft-anything                # masaüstü uygulaması
cargo install --path apps/craft-anything-cli         # CLI, `craft-anything-cli` olarak
craft-anything-cli doctor                            # kurulumu, kodlama ajanlarını ve motorları kontrol eder
```

Yapım için bilgisayarında giriş yapılmış bir kodlama ajanı gerekir (bkz. [Yapay zekâ](#yapay-zekâ)); `doctor` neyin
eksik olduğunu söyler. Komut satırından yeniden yapım (sorular terminalde sorulur):

```sh
craft-anything-cli rebuild https://example.com "Recreate this website"
craft-anything-cli rebuild - "A snake game in Rust" --rounds 1
craft-anything-cli rebuild https://example.com "Make the header blue" --continue-in "<proje klasörü>"
craft-anything-cli rebuild - "Add a contact form" --project-folder "<senin klasörün>" --existing edit
```

Yeni projeler `Documents/Craft Anything` klasörüne gider (Ayarlar → General'dan değiştirilebilir). Her proje
klasörü şunları tutar: `craft.json` (istek, hedef, durum ve geçmiş), `capture/` (hedeften kaydedilenler),
`app/` (üretilen proje), `checks/` (orijinalle yapılan her karşılaştırma), `investigation.cany/` (kanıtlar)
ve `inbox/` (açık sorular).

Bir proje senin seçtiğin bir klasörde de başlayabilir (Start altında Choose a folder… ya da `--project-folder`).
Klasörde zaten dosyalar varsa ne olduklarını sen söylersin: yapıcının yerinde düzenleyeceği uygulamanın kendisi mi,
yoksa başlangıç malzemesi mi (bu durumda uygulama yanlarındaki `app/` klasöründe yapılır). Ayarlar → General bu
cevabı hatırlayabilir. Ayarlar → General ayrıca başka klasörlerdeki projeleri de listeler; birini kaldırmak
onu yalnızca listeden çıkarır.

## Durum

> [!IMPORTANT]
> **Durum:** Craft Anything erken alfa aşamasında. Bugün ne çalışıyor, ne kısmen var ve sırada ne var:
>
> - **Bugün çalışan:** masaüstü uygulaması (Basit ve Gelişmiş), CLI ve MCP sunucusu; yapıcı olarak Claude Code ile
>   web siteleri, uygulamalar ve oyunlar için Study → Build → Check → Ready akışı; üç ekran genişliğinde
>   web sitesi kontrolleri; Unity, Unreal veya Godot'da oyun yeniden yapımları; Ghidra ile Deep modu;
>   üretilmiş resimler ve 3D modeller (isteğe bağlı, kendi fal.ai anahtarınla); eklentiler (MCP
>   sunucuları ve `wasm-plugins` seçeneğiyle derlenmiş sürümlerde WASM bileşenleri).
> - **Kısmen var:** API anahtarları ve diğer kodlama ajanlarıyla yapım (yapıldı, henüz canlı çalıştırılmadı),
>   uygulamalar ve oyunlar için davranış kontrolleri, yalnızca yedek hedeflere karşı test edilmiş birkaç motor aracı.
> - **Yayınlandı:** Windows, macOS, Linux ve FreeBSD için kurulum dosyalı genel sürümler
>   [Releases sayfasında](https://github.com/storytold/craft-anything/releases) yayınlandı; hâlâ alfa aşamasındalar.
> - **Sırada:** yönetilen (managed) katman.
>
> Ayrıntılar ve her parçanın nasıl kontrol edildiği [docs/STATUS.md](docs/STATUS.md),
> araç araç kontrol listesi [docs/tool-status.md](docs/tool-status.md) ve
> madde madde plan [docs/ROADMAP.md](docs/ROADMAP.md) belgesinde. Lütfen issue aç ya da neyin
> bozulduğunu [Discord](https://discord.gg/artcraft)'da anlat.

**Sorumlu kullanım.** Craft Anything'i sahibi olduğun ya da incelemene izin verilen yazılımlarda kullan. DRM'yi,
girişleri veya hile korumalarını (anti-cheat) asla atlatmaz. Yeniden yapımlar; yerleşimi, stili ve davranışı kanıtlardan
yeniden oluşturur. Ajana, sahibi olduğunu ya da iznin olduğunu onaylamadıkça logoları, fotoğrafları veya metinleri
kopyalamaması söylenir.

**Bağımlılık kurmak kod çalıştırır.** Bir yapıcı bir projenin bağımlılıklarını kurduğunda komutlar
senin bilgisayarında çalışır ve paketler kurulurken kod çalıştırabilir. npm varsayılan olarak her paketin
kurulum betiklerini çalıştırır; Rust derleme betikleri ve Python kaynak paketleri de kendi kurulum kodlarını çalıştırır.
Craft Anything, bu projede daha önce görmediği türde bir komuttan (`npm`, `cargo`…) önce sorar.
`npm` için "always allow" seçmek, sonraki tüm npm komutlarının sormadan çalışmasına izin verir; bunu yalnızca
bağımlılıklarına güvendiğin bir proje için yap. npm paketlerini kurulum betikleri olmadan kurmak için
`npm install --ignore-scripts` çalıştır.

<a id="indirmeler"></a>

## İndirmeler

GitHub'da [son sürüm](https://github.com/storytold/craft-anything/releases/latest) aşağıdaki tüm derlemeleri içerir; [tüm sürümler](https://github.com/storytold/craft-anything/releases) sayfasında eski sürümler ve notları var. Dosya adlarındaki `<ver>` sürüm numarasıdır; `SHA256SUMS.txt` her dosyanın sağlama toplamını listeler.

### Windows

| Derleme | Kurulum dosyası | Taşınabilir |
|---|---|---|
| x64 (64-bit Intel/AMD) | `craft-anything-<ver>-windows-x64.msi` | `craft-anything-<ver>-windows-x64-portable.zip` |
| arm64 (Snapdragon ve diğer ARM bilgisayarlar) | `craft-anything-<ver>-windows-arm64.msi` | `craft-anything-<ver>-windows-arm64-portable.zip` |
| x86 (32-bit) | `craft-anything-<ver>-windows-x86.msi` | `craft-anything-<ver>-windows-x86-portable.zip` |

Kurulum dosyaları ve çalıştırılabilir dosyalar kod imzalıdır. Taşınabilir zip, ayarlarını uygulamanın yanında tutar.

### macOS

| Derleme | Dosya | Notlar |
|---|---|---|
| Uygulama, evrensel (Apple silicon + Intel) | `craft-anything-<ver>-macos-universal.dmg` | İmzalı ve Apple onaylı (notarized) |
| Komut satırı aracı, evrensel | `craft-anything-cli-<ver>-macos-universal.zip` | İmzalı ve Apple onaylı; macOS engellerse aşağıya bak |

0.2.0'dan sonraki sürümler komut satırı aracını uygulamanın içinde getirir, yani ayrı zip'e gerek yoktur.
Ona yalnızca komut satırını uygulama olmadan kullanmak istiyorsan ihtiyacın var.

Zip'i tarayıcıdan indirdiğinde "Apple could not verify craft-anything-cli" hatasıyla durabilir (tek başına bir
program onay bilgisini yanında taşıyamaz, bu yüzden macOS'un bunu internetten kontrol etmesi gerekir). Terminalden
yapılan indirme bu şekilde kontrol edilmez, bu yüzden bunun yerine şunu kullan:

```sh
ver=0.2.0   # bunu istediğin sürümle değiştir
curl -fLO "https://github.com/storytold/craft-anything/releases/download/v$ver/craft-anything-cli-$ver-macos-universal.zip"
ditto -x -k "craft-anything-cli-$ver-macos-universal.zip" .
mkdir -p ~/.local/bin
mv "craft-anything-cli-$ver-macos-universal/craft-anything-cli" ~/.local/bin/
~/.local/bin/craft-anything-cli --version
```

Sadece `craft-anything-cli` yazarak çalıştırmak için `~/.local/bin` klasörünü `PATH`'ine ekle.

Engellenmiş kopya zaten sendeyse Done'a bas (Move to Bin değil), sonra
`xattr -d com.apple.quarantine <craft-anything-cli yolu>` çalıştır ya da Sistem Ayarları →
Gizlilik ve Güvenlik → Yine de Aç bölümünden izin ver.

#### Mac indirmesinin imzalı ve onaylı olduğunu kontrol etmek

Her Mac sürümü bir Developer ID sertifikasıyla imzalanır ve Apple tarafından onaylanır. Bir indirmeyi
kendin kontrol etmek için bunları Terminal'de çalıştır (`<ver>` yerine sürümü, yollar yerine kendi yollarını yaz):

```sh
# Disk imajı: "accepted" ve "source=Notarized Developer ID" geçtiği anlamına gelir
spctl --assess --type open --context context:primary-signature -v "craft-anything-<ver>-macos-universal.dmg"
# Onay bileti ekli: "The validate action worked!" yazdırır
xcrun stapler validate "craft-anything-<ver>-macos-universal.dmg"

# Uygulama, disk imajını açtıktan sonra: imza doğrulanmalı
codesign --verify --deep --strict --verbose=2 "/Volumes/<disk imajı adı>/Craft Anything.app"
spctl --assess --type execute --verbose "/Volumes/<disk imajı adı>/Craft Anything.app"

# Komut satırı aracı: tek başına bir program bilet taşıyamaz, bu yüzden macOS internetten kontrol eder
codesign --verify --strict --verbose=2 ~/.local/bin/craft-anything-cli
codesign -dv --verbose=4 ~/.local/bin/craft-anything-cli 2>&1 | grep Authority
```

`Authority` satırları bir "Developer ID Application" sertifikasını göstermeli. Bir kontrol başarısız olursa dosyayı
çalıştırma; çıktıyla ve sürüm etiketiyle birlikte bildir.

### Linux

| Format | x86_64 | aarch64 (ARM64) | Notlar |
|---|---|---|---|
| AppImage | `craft-anything-<ver>-linux-x86_64.AppImage` | `craft-anything-<ver>-linux-aarch64.AppImage` | Her yerde çalışır; [AppImageUpdate](https://github.com/AppImageCommunity/AppImageUpdate) ile kendini günceller (`.zsync` dosyaları) |
| Flatpak | `craft-anything-<ver>-linux-x86_64.flatpak` | `craft-anything-<ver>-linux-aarch64.flatpak` | Sandbox içinde çalıştığı için bilgisayarında tarayıcı başlatamaz, web sitelerini incelemek ise bunu gerektirir (bunun için başka bir paket kullan); `flatpak install --user <dosya>` |
| Debian/Ubuntu | `craft-anything-<ver>-linux-x86_64.deb` | `craft-anything-<ver>-linux-aarch64.deb` | |
| Fedora/RHEL/openSUSE | `craft-anything-<ver>-linux-x86_64.rpm` | `craft-anything-<ver>-linux-aarch64.rpm` | |
| Tarball | `craft-anything-<ver>-linux-x86_64.tar.gz` | `craft-anything-<ver>-linux-aarch64.tar.gz` | İstediğin yere aç |

RISC-V (riscv64): `craft-anything-<ver>-linux-riscv64.tar.gz`, yalnızca tarball.

AppImage CLI'ı da içerir: `<AppImage> --cli <komut>` çalıştır (örneğin bir MCP istemcisi için
`<AppImage> --cli mcp`) ya da AppImage'a `craft-anything-cli` adlı bir bağlantı oluştur ve onu çalıştır.

### FreeBSD

| Derleme | Dosya |
|---|---|
| x86_64 | `craft-anything-<ver>-freebsd-x86_64.tar.gz` |

### Web (WebAssembly)

| Derleme | Dosya | Notlar |
|---|---|---|
| Tarayıcı görüntüleyici | `craft-anything-web-<ver>.zip` | Bir dosyanın ne olduğunu tarayıcıda söyler (dosya tarayıcıdan hiç çıkmaz); klasörü herhangi bir statik web sunucusunda barındır ([packaging/web/README.md](packaging/web/README.md)) |

<br>

<p align="center">
  <a href="https://discord.gg/artcraft"><img alt="Discord'da ArtCraft topluluğuna katıl" src="https://img.shields.io/badge/Join%20us%20on%20Discord-5865F2?style=for-the-badge&logo=discord&logoColor=white" height="40"></a>
</p>

<h3 align="center">Gel, birlikte bir şeyler yapalım</h3>

<p align="center">
  Discord'umuz her türden sanatçının takıldığı yer: resim yapanlar, fotoğraf çekenler, çizenler, film kurgulayanlar,
  dizgi yapanlar ve neyi üretmeyi sevdiğini hâlâ keşfedenler. Üzerinde çalıştığın şeyi paylaş,
  yardım iste, neyin bozuk olduğunu ya da bu araçların neler yapabilmesini istediğini anlat.
  Hangi alanda olursan ol, ne kadar zamandır uğraşıyor olursan ol, burada hoş karşılanırsın.
</p>

<p align="center">
  <a href="https://discord.gg/artcraft"><b>discord.gg/artcraft</b></a> ·
  <a href="https://getartcraft.com/">getartcraft.com</a> ·
  <a href="https://getartcraft.com/apps">Craft uygulamaları</a> ·
  <a href="https://github.com/storytold/craft-anything">Craft Anything</a>
</p>

---

## Craft Uygulama Paketi

| | Uygulama | Ne için | Kod | Daha fazla bilgi |
|:-:|---|---|---|---|
| <img src="https://raw.githubusercontent.com/storytold/photocraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.photocraft.png" alt="" width="32" height="32"> | **PhotoCraft** | Görsel düzenleme: katmanlar, maskeler, yazı ve gerçek PSD dosyaları | [GitHub](https://github.com/storytold/photocraft) | [Web sitesi](https://getartcraft.com/apps/photocraft) |
| <img src="https://raw.githubusercontent.com/storytold/vectorcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.vectorcraft.png" alt="" width="32" height="32"> | **VectorCraft** | Vektör illüstrasyon | [GitHub](https://github.com/storytold/vectorcraft) | [Web sitesi](https://getartcraft.com/apps/vectorcraft) |
| <img src="https://raw.githubusercontent.com/storytold/filmcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.filmcraft.png" alt="" width="32" height="32"> | **FilmCraft** | Video düzenleme, renk ve ses | [GitHub](https://github.com/storytold/filmcraft) | [Web sitesi](https://getartcraft.com/apps/filmcraft) |
| <img src="https://raw.githubusercontent.com/storytold/lightcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.lightcraft.png" alt="" width="32" height="32"> | **LightCraft** | Fotoğraf kütüphanesi ve RAW banyo | [GitHub](https://github.com/storytold/lightcraft) | [Web sitesi](https://getartcraft.com/apps/lightcraft) |
| <img src="https://raw.githubusercontent.com/storytold/pdfcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.pdfcraft.png" alt="" width="32" height="32"> | **PdfCraft** | PDF okuma, düzenleme ve koruma | [GitHub](https://github.com/storytold/pdfcraft) | [Web sitesi](https://getartcraft.com/apps/pdfcraft) |
| <img src="https://raw.githubusercontent.com/storytold/effectcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.effectcraft.png" alt="" width="32" height="32"> | **EffectCraft** | Hareketli grafik ve görsel efektler | [GitHub](https://github.com/storytold/effectcraft) | [Web sitesi](https://getartcraft.com/apps/effectcraft) |
| <img src="https://raw.githubusercontent.com/storytold/designcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.designcraft.png" alt="" width="32" height="32"> | **DesignCraft** | Sayfa düzeni ve yayıncılık | [GitHub](https://github.com/storytold/designcraft) | [Web sitesi](https://getartcraft.com/apps/designcraft) |
| <img src="https://raw.githubusercontent.com/storytold/figcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.figcraft.png" alt="" width="32" height="32"> | **FigCraft** | Arayüz tasarımı ve prototipleme | [GitHub](https://github.com/storytold/figcraft) | [Web sitesi](https://getartcraft.com/apps/figcraft) |
| <img src="https://raw.githubusercontent.com/storytold/wordcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.wordcraft.png" alt="" width="32" height="32"> | **WordCraft** | Kelime işlem ve belgeler | [GitHub](https://github.com/storytold/wordcraft) | [Web sitesi](https://getartcraft.com/apps/wordcraft) |
| <img src="https://raw.githubusercontent.com/storytold/soundcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.soundcraft.png" alt="" width="32" height="32"> | **SoundCraft** | Ses kaydı ve düzenleme | [GitHub](https://github.com/storytold/soundcraft) | [Web sitesi](https://getartcraft.com/apps/soundcraft) |
| <img src="https://raw.githubusercontent.com/storytold/deckcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.deckcraft.png" alt="" width="32" height="32"> | **DeckCraft** | Sunumlar ve slaytlar | [GitHub](https://github.com/storytold/deckcraft) | [Web sitesi](https://getartcraft.com/apps/deckcraft) |
| <img src="https://raw.githubusercontent.com/storytold/cadcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.cadcraft.png" alt="" width="32" height="32"> | **CADCraft** | Bilgisayar destekli tasarım | [GitHub](https://github.com/storytold/cadcraft) | [Web sitesi](https://getartcraft.com/apps/cadcraft) |
| <img src="https://raw.githubusercontent.com/storytold/gridcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.gridcraft.png" alt="" width="32" height="32"> | **GridCraft** | Hesap tabloları ve veri | [GitHub](https://github.com/storytold/gridcraft) | [Web sitesi](https://getartcraft.com/apps/gridcraft) |
| <img src="https://raw.githubusercontent.com/storytold/texturecraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.texturecraft.png" alt="" width="32" height="32"> | **TextureCraft** | 3D doku boyama | [GitHub](https://github.com/storytold/texturecraft) | [Web sitesi](https://getartcraft.com/apps/texturecraft) |
| <img src="https://raw.githubusercontent.com/storytold/materialcraft/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.materialcraft.png" alt="" width="32" height="32"> | **MaterialCraft** | Düğüm tabanlı prosedürel malzemeler | [GitHub](https://github.com/storytold/materialcraft) | [Web sitesi](https://getartcraft.com/apps/materialcraft) |
| <img src="https://raw.githubusercontent.com/storytold/craft-anything/main/assets/app-icon/hicolor/64x64/apps/ai.storyteller.craft-anything.png" alt="" width="32" height="32"> | **Craft Anything** | Herhangi bir yazılımın nasıl çalıştığını gör, sonra kendininkini yap | [GitHub](https://github.com/storytold/craft-anything) | [Web sitesi](https://getartcraft.com/apps/craft-anything) |
| <img src="https://raw.githubusercontent.com/tangency-labs/solidcraft/main/assets/icons/app/linux/exports/hicolor/64x64/apps/io.tangencylabs.solidcraft.png" alt="" width="32" height="32"> | **SolidCraft** | Tangency Labs'ten parametrik 3D CAD (topluluk uygulaması) | [GitHub](https://github.com/tangency-labs/solidcraft) | |


## Lisans ve emeği geçenler

Craft Anything, tercihine göre [MIT](LICENSE-MIT) veya [Apache-2.0](LICENSE-APACHE) lisanslarıyla çift lisanslıdır.
Telif hakkı (c) 2026 ArtCraft Team ve Craft Anything katkıcıları ([CONTRIBUTORS.md](CONTRIBUTORS.md)). Gerekli bildirimler [NOTICE](NOTICE) dosyasındadır.

Pakete dahil yazı tipleri, simgeler, resimler ve diğer varlıklar kendi açık lisanslarını korur; her biri
yazarı, kaynağı ve lisansıyla [ATTRIBUTION.md](ATTRIBUTION.md) dosyasında listelenmiştir.

Ekran görüntüleri bunlar için yapılmış örnek projeleri gösterir: example.com, bir yılan oyunu ve küçük bir
hesap makinesi programı.

[`docs/brand/`](docs/brand/) içindeki ArtCraft adı, yazı logosu ve logolar ArtCraft Team'in ticari markalarıdır
ve bu lisansın kapsamında değildir. Yalnızca değiştirilmeden, yalnızca bu deponun ve Craft Anything'in bir parçası
olarak, [`docs/brand/LICENSE-brand.txt`](docs/brand/LICENSE-brand.txt) koşullarıyla kullanılabilirler.
Fork'lar ve değiştirilmiş sürümler bunları kaldırmalıdır.

<p align="center">
  <a href="https://getartcraft.com/"><img alt="ArtCraft" src="docs/brand/artcraft-mark.svg" width="28"></a><br>
  <sub><a href="https://getartcraft.com/">ArtCraft</a> ekibi ve topluluğu tarafından yapıldı.</sub>
</p>
