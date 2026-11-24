# Publicar uma versão do Musaeum

Como sai uma versão para Windows: o que o instalador leva, como construí-lo de novo a partir do
repositório, como conferi-lo fora da máquina de desenvolvimento e como assiná-lo para uma publicação
pública. As decisões estão no ADR 0015.

## O que sai

| Arquivo | Para quem |
|---|---|
| `target/release/bundle/nsis/Musaeum_1.0.0_x64-setup.exe` | Uma pessoa: instala só para o usuário, sem pedir administrador, em português do Brasil ou em inglês |
| `target/release/bundle/msi/Musaeum_1.0.0_x64_pt-BR.msi` e `..._en-US.msi` | Quem administra várias máquinas (GPO, Intune): instala para a máquina toda e pede administrador |

Os três levam os mesmos arquivos, ao lado do executável:

- `musaeum-desktop.exe`, o app;
- os seis workers, cada um no seu sandbox: `musaeum-render-pdf.exe`, `musaeum-pdf-annots.exe`,
  `musaeum-render-raster.exe`, `musaeum-render-djvu.exe`, `musaeum-ocr.exe` e `musaeum-ingest.exe`;
- `pdfium.dll`, a build fixada do PDFium;
- `tessdata\por.traineddata` e `tessdata\eng.traineddata`, com o SHA-256 que o app confere antes de
  usá-los, e `tessdata\LICENSE`;
- `LICENSE.txt` (GPL-3.0) e `THIRD-PARTY-NOTICES.md`, os avisos e as licenças de tudo o que é de
  terceiros.

As fontes Noto dos relatórios vão dentro do executável. O runtime do C e do C++ vai dentro de cada
executável (`.cargo/config.toml`): nenhum binário pede o Visual C++ Redistributable, e
`tools/stage-bundle.mjs` recusa montar um instalador em que algum peça.

O que a máquina precisa ter: Windows 10 (1809 ou mais novo) ou 11, x64, e o WebView2, que já vem no
Windows 11. Numa máquina sem ele, o instalador baixa o instalador oficial da Microsoft: é o único acesso à
rede, e só na instalação. O app nunca acessa a rede.

Onde fica cada coisa depois de instalado:

- o programa, em `%LOCALAPPDATA%\Musaeum` (NSIS) ou em `C:\Program Files\Musaeum` (MSI);
- a biblioteca e o cache, em `%LOCALAPPDATA%\Musaeum\library` e `...\cache`, e as preferências em
  `%LOCALAPPDATA%\Musaeum\preferences.db`, criados na primeira vez que são preciso;
- o perfil do WebView2, em `%LOCALAPPDATA%\org.musaeum.desktop`.

Desinstalar apaga os arquivos do programa e nada da biblioteca, das anotações ou das preferências. Pelo
NSIS, a desinstalação silenciosa deixa em `HKCU\Software\Musaeum\Musaeum` a pasta em que o programa
estava, para que uma reinstalação vá ao mesmo lugar; a interativa a apaga quando a pessoa marca "apagar
os dados do aplicativo", que apaga também o perfil do WebView2, nunca a biblioteca.

## Construir

Na máquina de build: Rust (a versão de `rust-toolchain.toml`), o Visual Studio Build Tools com C++ e o
Windows SDK, Node.js 24 e o `cargo-deny`. O NSIS e o WiX não precisam estar instalados: o
`tauri build` baixa as versões que o `@tauri-apps/cli` 2.12.4 fixa, confere o hash delas e as guarda em
`%LOCALAPPDATA%\tauri`.

```powershell
powershell -ExecutionPolicy Bypass -File tools\fetch-pdfium.ps1
powershell -ExecutionPolicy Bypass -File tools\fetch-ocr.ps1
powershell -ExecutionPolicy Bypass -File tools\fetch-fonts.ps1

cd apps\desktop
npm ci
npm run build:bundle
cd ..\..
```

`npm run build:bundle` faz, em ordem:

1. `npm run build:app`: os workers, o `musaeum-ingest` e o app, em release;
2. `node tools\stage-bundle.mjs`: junta em `target\bundle-stage` o que vai ao lado do app, e recusa se um
   binário pede uma DLL que o Windows não tem, se um modelo do OCR não tem o hash fixado, ou se o
   `THIRD-PARTY-NOTICES.md` não está em dia;
3. `tauri build --config src-tauri\tauri.bundle.json`: o app de novo, com a configuração do instalador, e
   o NSIS e os dois MSIs.

A configuração do instalador fica em `tauri.bundle.json`, à parte do `tauri.conf.json`, porque o
`tauri-build` copia os `resources` para a pasta do build a cada compilação: com eles no arquivo principal,
um `cargo build` qualquer, também o dos testes, poria os workers de release no lugar dos de debug.

O que é fixado, e portanto o mesmo em qualquer máquina: o Rust (`rust-toolchain.toml`), cada crate
(`Cargo.lock`, `--locked`), cada pacote npm (`package-lock.json`, `npm ci`), o PDFium, o Tesseract, o
Leptonica, os modelos e as fontes (pelo SHA-256, nos scripts `fetch-*`), e o NSIS e o WiX (pela versão
do `@tauri-apps/cli`). O que não é igual byte a byte de um build para outro: as datas que o linker do
MSVC grava nos executáveis, o código de pacote que o WiX sorteia para cada MSI e as datas dentro do
instalador NSIS. Por isso o que se confere de um build para outro é o conteúdo, como o teste de fumaça
abaixo faz, não o hash do instalador.

`THIRD-PARTY-NOTICES.md` é escrito por `node tools\third-party-notices.mjs` a partir do `Cargo.lock`, do
`package-lock.json` e do `vendor\third-party.json`. A conferência de licenças (`node
tools\check-licenses.mjs`, no hook de pre-commit e no CI) falha se ele não estiver em dia.

## Conferir

```powershell
cargo build --release -p musaeum-cli -p visual-check -p sandbox-audit -p gen-fixtures
node tools\smoke-bundle.mjs
node tools\e2e-ipc.mjs
cargo deny check advisories
```

`tools\smoke-bundle.mjs` instala o NSIS numa pasta temporária (silencioso, por usuário, sem atalhos),
confere os arquivos, os hashes dos modelos, as DLLs que cada binário pede e a versão
gravada no executável, abre o app instalado com só as pastas do Windows no `PATH` e a biblioteca e o perfil
do WebView2 também na pasta temporária, abre um quadrinho, um DjVu, um EPUB, um PDF e um escaneado,
lê uma caixa do escaneado pelo OCR, exporta JSON-LD, um relatório em PDF e uma cópia do PDF, audita de
fora o sandbox de cada worker vivo (`tools\sandbox-audit`), fecha o app, desinstala e confere que nada
ficou: nem processo, nem registro, nem atalho, nem arquivo em `%TEMP%`. Depois lê as tabelas de cada MSI
sem instalar, desempacota uma imagem administrativa (só os arquivos, nada registrado), confere os mesmos
arquivos byte a byte e abre um PDF com o app dela. Ele se recusa a começar se houver um Musaeum aberto
(o instalador o encerraria) ou instalado, e espera a máquina livre.

`tools\e2e-ipc.mjs` abre o app como uma pessoa o abre, sem `--e2e`, e pede ao host, da própria página,
tudo o que uma página hostil pediria: os comandos do Tauri que a página não usa, abrir um arquivo fora da
biblioteca, seguir uma pasta, ler uma biblioteca do Calibre, gravar onde o diálogo não deu. Tudo é
recusado e nada muda no disco.

Publique o SHA-256 de cada instalador junto com ele:

```powershell
Get-FileHash target\release\bundle\nsis\*.exe, target\release\bundle\msi\*.msi -Algorithm SHA256
```

## Assinar, para uma publicação pública

Um instalador sem assinatura funciona, mas o Windows (SmartScreen) avisa quem o baixa, e nada prova que
ele é o que saiu deste repositório. O build local não precisa de certificado nenhum; uma publicação
pública precisa de um certificado de assinatura de código: OV ou EV de uma autoridade certificadora (o EV
vem num token de hardware ou num HSM) ou o Azure Trusted Signing.

O que assinar: o app, os seis workers, o `pdfium.dll` (que o projeto pdfium-binaries publica sem
assinatura), o desinstalador e os instaladores. O Tauri assina o app, o desinstalador e os instaladores
quando configurado, mas não assina os `resources`: os workers e o `pdfium.dll` são assinados na pasta de
montagem, antes do `tauri build`. A assinatura não muda o sandbox: os workers de quadrinhos, DjVu, OCR e
ingestão só carregam DLLs do Windows, que a Microsoft assina, e os do PDFium não exigem DLLs assinadas
pela Microsoft.

Com o certificado instalado no repositório de certificados do usuário (`Cert:\CurrentUser\My`), e
`THUMBPRINT` a impressão digital dele:

```powershell
$signtool = "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe"
$timestamp = 'http://timestamp.digicert.com'

cd apps\desktop
npm run build:app
node ..\..\tools\stage-bundle.mjs
cd ..\..

# Os workers e o pdfium.dll, já na pasta de montagem.
& $signtool sign /sha1 THUMBPRINT /fd SHA256 /tr $timestamp /td SHA256 /d Musaeum `
  target\bundle-stage\*.exe target\bundle-stage\pdfium.dll
& $signtool verify /pa /all target\bundle-stage\*.exe target\bundle-stage\pdfium.dll

# O app, o desinstalador e os instaladores, pelo Tauri.
cd apps\desktop
npx tauri build --config src-tauri\tauri.bundle.json --config src-tauri\tauri.sign.json
cd ..\..
& $signtool verify /pa /all target\release\bundle\nsis\*.exe target\release\bundle\msi\*.msi
```

com `apps\desktop\src-tauri\tauri.sign.json`, que não vai para o repositório:

```json
{
  "bundle": {
    "windows": {
      "certificateThumbprint": "THUMBPRINT",
      "digestAlgorithm": "sha256",
      "timestampUrl": "http://timestamp.digicert.com"
    }
  }
}
```

Não rode o `npm run build:bundle` depois de assinar a pasta de montagem: ele a monta de novo, sem as
assinaturas. Com um certificado num HSM ou no Azure Trusted Signing, o `signtool` recebe as opções do
fornecedor (`/dlib`, `/dmdf`) no lugar de `/sha1`, e o `tauri.sign.json` usa `signCommand` no lugar de
`certificateThumbprint`, com `%1` onde vai o arquivo.

Depois de assinar, rode o teste de fumaça de novo, e confira as assinaturas instaladas:

```powershell
Get-AuthenticodeSignature (Join-Path $env:LOCALAPPDATA 'Musaeum\*.exe') | Format-Table Status, Path
```

## Depois de publicar

- A tag anotada `vX.Y.Z` no commit do build, e o `CHANGELOG.md` com a versão e a data.
- O SHA-256 dos instaladores na página da versão.
- O código de atualização dos MSIs (`UpgradeCode`, `038bf292-ffad-5c1b-985a-dbcbe8fd0452`) fica
  fixado em `tauri.bundle.json` e não muda entre versões: é ele que faz um MSI novo substituir o antigo.
  É o UUID v5 que o Tauri derivaria de `Musaeum.exe.app.x64`, fixado para não depender do nome do
  produto. O NSIS se reconhece pela chave `Musaeum` em
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall`.
