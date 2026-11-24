# ADR 0015: decisões da Fase 8, Parte 2, a auditoria final de segurança e a versão 1.0.0

| | |
|---|---|
| **Data** | 2026-11-24 |
| **Situação** | Aceita na auditoria de 2026-11-24, sem emendas (seção 10) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 158, Windhawk 1.7.3 instalado |
| **Escopo** | A auditoria de ponta a ponta do sandbox dos workers, da página do app (CSP, capacidades do Tauri) e de cada comando do IPC; as dependências, as vulnerabilidades conhecidas e os avisos de terceiros; a versão 1.0.0 nos manifestos; os instaladores NSIS e MSI, conferidos fora do repositório; a documentação de lançamento; a bateria final |

## 1. Resumo

- **A auditoria de segurança achou e fechou cinco portas.** A página do app podia abrir qualquer arquivo
  do disco como documento (e ler o texto dele), seguir qualquer pasta, ler qualquer biblioteca do Calibre
  e usar comandos do próprio Tauri que não usa; e a biblioteca aceitava como pasta de livros a pasta dos
  seus próprios arquivos. Agora o host só abre o que a biblioteca conhece ou o que o diálogo do sistema
  deu, e a página só tem do Tauri a escuta de eventos. Uma passada nova abre o app como uma pessoa o abre
  e tenta cada uma dessas portas: 36 de 36 recusas.
- **Cada worker é conferido de fora**, nos testes e no app instalado: o token, as mitigações, o job, cada
  handle e cada DLL. Os seis passam. Nesta máquina, o Windhawk (um programa de personalização que injeta
  código em todo processo) entra por milissegundos nos dois workers do PDFium ao nascerem; os outros
  quatro o barram. Está documentado com as mitigações avaliadas (seção 8, ponto 4).
- **O instalador roda numa máquina sem nada de desenvolvimento**: os workers importavam o runtime do
  Visual C++ e não começariam sem ele; agora ele vai dentro de cada executável. Os instaladores (NSIS por
  usuário, e um MSI por idioma para a máquina) foram instalados, usados, auditados e desinstalados fora
  do repositório, sem deixar nada.
- **A versão é 1.0.0**, com avisos de terceiros gerados e conferidos, três dependências atualizadas por
  avisos de segurança, e a documentação de lançamento. A bateria inteira passou no build final: 851
  testes de Rust, 232 do Vitest, as 7 passadas na janela, a do IPC e a do instalador; o acervo, lido
  antes e depois, não mudou um byte.

## 2. Entregas

| Onde | O quê |
|---|---|
| `crates/musaeum-sandbox/src/audit.rs` | `ProcessAudit`: lê de fora, como o kernel descreve, o token, as mitigações, cada handle de um processo (o nome do tipo de cada um, também dos que não se copiam, e o executável dono de um handle de processo), cada DLL carregada e a idade do processo, e lista o que fica aquém da política dele; `SandboxReport::violations` faz o mesmo com o job e o token de partida; os testes das regras |
| `crates/*/tests/sandbox.rs` | Um teste por worker que o inicia como o app o inicia (a política e o limite de memória do código de produção) e exige zero violações: render de PDF e anotações do PDF, quadrinhos, DjVu, OCR (antes e depois de carregar os modelos); a ingestão, antes e depois de ler um livro |
| `tools/sandbox-audit/` | A mesma auditoria nos workers vivos de um app em execução, o instalado inclusive, com a política de cada um tirada do código de produção; `--follow-ms` acompanha um worker de vida curta, e cada leitura diz se ele já tinha feito o lockdown |
| `tools/e2e-ipc.mjs` | O app aberto como uma pessoa o abre, sem `--e2e`, e, da própria página, cada pedido que uma página hostil faria |
| `apps/desktop/src-tauri/src/{session,shelf,calibre}.rs` | `open_session` só abre um livro da biblioteca ou o documento da linha de comando, com no máximo 16 workers; `library_add_root` só sob teste; o Calibre, só a pasta que o diálogo deu |
| `crates/musaeum-library/src/library.rs` | A recusa de acompanhar uma pasta que contém os arquivos da biblioteca, ou está dentro deles |
| `apps/desktop/src-tauri/capabilities/default.json` | Das permissões do Tauri, só ouvir eventos |
| `apps/desktop/src-tauri/tauri.conf.json`, `src/book.rs` | A CSP da página com `object-src`, `base-uri` e `form-action`; a do leitor fluido sem o servidor de desenvolvimento num build de release |
| `.cargo/config.toml` | O runtime do C e do C++ ligado estaticamente em todos os executáveis |
| `Cargo.toml`, `package.json`, `package-lock.json`, `tauri.conf.json` | A versão 1.0.0 |
| `apps/desktop/src-tauri/tauri.bundle.json` | A configuração dos instaladores, à parte, usada só no empacotamento |
| `apps/desktop/src-tauri/icons/icon.ico` | O mesmo ícone, com as camadas de 16, 24, 32, 48, 64 e 256 px |
| `tools/stage-bundle.mjs`, `tools/lib/pe.mjs` | A pasta de montagem do instalador, e a recusa de um binário que peça uma DLL que o Windows não tem |
| `tools/smoke-bundle.mjs`, `tools/lib/msi-tables.ps1` | O teste de fumaça dos instaladores, fora do repositório |
| `tools/third-party-notices.mjs`, `THIRD-PARTY-NOTICES.md` | Os avisos e as licenças de terceiros, conferidos pela verificação de licenças |
| `apps/cli` | `musaeum --version` |
| `CHANGELOG.md`, `docs/release.md`, `README.md` | A documentação de lançamento |
| `.github/workflows/ci.yml` | `cargo deny check advisories` e `npm audit` no CI |

## 3. A auditoria de segurança

### 3.1 Os workers

Cada processo que interpreta bytes de um livro foi iniciado como o app o inicia, com a política e o limite
de memória do código de produção (`FixedFormat::policy`, `OcrConfig`, `AnnotWorkerConfig`,
`IngestConfig`), e lido de fora pelo `ProcessAudit`:

| Worker | Política | win32k | Só DLLs da Microsoft | Limite de memória | Arquivos que segura, além dos 3 pipes |
|---|---|---|---|---|---|
| `musaeum-render-pdf` | `Lockdown` | alcançável | não | 2 GiB no leitor, 1 GiB em segundo plano | o livro (`0x120089`, só leitura), `System32` e a pasta do GDI+ no WinSxS (só travessia), `\Device\CNG` |
| `musaeum-pdf-annots` | `Lockdown` | alcançável | não | 3 GiB | nenhum livro entre pedidos; `System32` e WinSxS (só travessia), `\Device\CNG` |
| `musaeum-render-raster` | `LockdownParser` | desligado | sim | 2 GiB no leitor, 1 GiB em segundo plano | o livro (só leitura), `System32` (só travessia), `\Device\CNG` e `\Device\KsecDD` |
| `musaeum-render-djvu` | `LockdownParser` | desligado | sim | 2 GiB no leitor, 1 GiB em segundo plano | `System32` (só travessia); o livro fica mapeado na memória, sem um handle aberto |
| `musaeum-ocr` | `LockdownParser` | desligado | sim | 768 MiB | `System32` (só travessia); os modelos chegam pelo pipe |
| `musaeum-ingest` | `LockdownParser` | desligado | sim | 128 MiB (3 GiB para normalizar) | nenhum livro entre pedidos: o handle de cada um é fechado antes da resposta |

Em todos: token restrito com o SID nulo como único SID restritivo, integridade baixa, nenhum privilégio, o
SID do usuário só para negar e só o rótulo de integridade utilizável; job que encerra o processo com o
host, não deixa criar outro processo e fecha a área de transferência, a área de trabalho e as janelas dos
outros; código gerado proibido, ASLR forçado, handles inválidos fatais, pontos de extensão desligados,
fontes só do sistema, imagens só locais e de integridade não baixa, CFG; o token de partida descartado. As
chaves do registro que eles seguram são só de leitura e nenhuma é da hive do usuário (as do carregador,
`Image File Execution Options` e `Sorting\Versions`, e nos do PDFium `HKLM` e `Microsoft\Ole`, abertas
pelo `user32`). A estação de janelas e a área de trabalho dos dois do PDFium são as do sandbox
(`Service-0x0-…$`, `\sandbox-<pid>`), nunca `WinSta0\Default`. Nenhum segura outro processo, um token,
uma seção com nome ou um arquivo que possa mudar. O resto da tabela são objetos do próprio processo:
eventos, temporizadores, o pool de threads, as portas de conclusão, o `\KnownDlls`, uma porta ALPC (a do
subsistema, que todo processo tem) e registros do ETW, que o kernel não deixa copiar e que a auditoria
identifica pelo nome do tipo.

**O que o app instalado mostrou nesta máquina: o Windhawk.** No teste de fumaça, a auditoria acompanhou o
worker das anotações do PDF, que o host inicia para um pedido e encerra depois, e o pegou nos primeiros
milissegundos com um handle de outro processo (acesso `0x101000`, sincronizar e consultar o mínimo) e um
`Mutant`. O dono do handle é o `windhawk.exe` (pid 4668, sessão 0), o serviço do Windhawk 1.7.3, um
programa de personalização do Windows instalado nesta máquina, que roda como SYSTEM e põe o seu motor
(`windhawk.dll`) em todo processo do usuário. O diagnóstico:

- o `musaeum-desktop.exe` tem o `windhawk.dll` e um mod dele carregados o tempo todo: o Windhawk o
  injetou como em qualquer outro programa (por qual caminho ele chega aos workers, o serviço ou o motor
  dentro do app, não foi verificado);
- num worker do PDF recém-criado, o `windhawk.dll` aparece nos primeiros milissegundos de vida, em parte
  dos workers, e some dentro de algumas dezenas de milissegundos. Na maioria das vezes ele some antes do
  lockdown. Uma vez, num worker de render, ele foi visto aos 12 ms **depois** do lockdown (sem o token de
  partida, e já sem a hive do usuário, que o lockdown fecha), e aos 69 ms já tinha saído: ali ele pode ter
  convivido com o começo da leitura do livro. Em regime, os workers do PDF têm só o executável e o
  `pdfium.dll` (amostras a cada poucos milissegundos, por 10 s);
- nos workers de `LockdownParser` (quadrinhos, DjVu, OCR, ingestão), o Windhawk nunca aparece: a política
  de só carregar DLLs assinadas pela Microsoft barra o `windhawk.dll`;
- nada do código do Musaeum abre ou duplica um processo para dentro de um worker.

O motor do Windhawk entra, portanto, só nos workers do PDFium, ao nascerem, e fica por alguns
milissegundos; o que ele segura ali é uma DLL dele e um handle que só sincroniza e consulta o mínimo do
serviço dele. Ele não dá ao worker nada que o sandbox negue (o token, o job e as mitigações são os mesmos),
mas é código de outro programa no processo, e a auditoria o acusa como tal. A seção 8 discute o que pode
evitá-lo.

As tentativas propriamente ditas (ler um arquivo pelo caminho, criar um, listar uma pasta, abrir o disco,
carregar uma DLL, conectar, escutar e abrir os drivers da rede, iniciar um processo, abrir o host, um pipe,
uma seção ou um evento com nome, ler o registro, a área de trabalho, a área de transferência e a tela,
gerar código executável e passar do limite de memória) são as do `sandbox-probe`, desde a Fase 0: todas
negadas no sandbox, e as que têm controle dão certo fora dele. O probe roda com o token de `Lockdown`, que é
o mesmo de `LockdownParser`: a diferença entre os dois é só de mitigações, que o `ProcessAudit` confere em
cada worker. O probe em si não roda sem o win32k, porque carrega o `user32` para tentar a tela e a área de
transferência.

### 3.2 A página do app, a CSP e as capacidades do Tauri

- **Capacidades.** A janela tinha `core:default`, que dá à página os comandos do próprio Tauri para a
  janela, as webviews, o app, os caminhos, os menus e a bandeja. A página só usa `listen` e `unlisten` dos
  eventos; agora só tem esses dois. Os comandos do app não passam pelas capacidades: cada um confere o que
  recebe (3.3). A origem de cada livro é remota para o Tauri e não tem capacidade nenhuma.
- **CSP da página.** `default-src 'self'`, scripts só do próprio app, nada de `eval`; ganhou
  `object-src 'none'`, `base-uri 'none'` e `form-action 'none'`. Os estilos inline continuam, pelo React e
  pelo React Aria.
- **CSP do leitor fluido** (desde a Fase 0): `default-src 'none'`, scripts só com o nonce do leitor, as
  seções em `blob:`, nada de rede além do próprio pacote. O `frame-ancestors` levava o servidor de
  desenvolvimento (`localhost:1420`) também no build de release; agora só no de desenvolvimento.
- **Protocolos próprios.** `thumb://` só serve uma capa por um hash de 64 dígitos hexadecimais; o dos
  livros, só os arquivos do manifesto do pacote normalizado, com o caminho decodificado estritamente; o dos
  tiles, só números de sessão e de slot, e só para as origens do próprio app. Os três têm testes de
  caminho hostil.
- **`--e2e` e `--bench`** continuam no binário de release, como sempre estiveram: abrem comandos que
  recebem caminhos, mas só quem já roda programas como o usuário pode passá-los, e o instalador não
  registra protocolo nem associação de arquivo que os pudesse passar. `--no-sandbox` e
  `--epub-protection` exigem `--bench`.

### 3.3 Os comandos do IPC

São 96 comandos. Os que recebem um caminho, ou algo que leva a um arquivo:

| Comando | Antes | Agora |
|---|---|---|
| `open_session` | abria qualquer caminho que a página desse, e o desenhava (e lia o seu texto) | só um livro da biblioteca ou o documento da linha de comando; a recusa vem antes de ler um byte e antes de fechar o livro aberto. Os workers pedidos ficam entre 1 e 16 |
| `library_add_root` | seguia qualquer pasta que a página desse | só sob `--e2e`; uma pessoa escolhe pastas no diálogo do sistema (`library_choose_root`) |
| `calibre_preview`, `calibre_import` | liam o `metadata.db` de qualquer pasta, e a seguiam | só a pasta que `calibre_choose` devolveu nesta execução (ou sob `--e2e`) |
| exportar e importar JSON-LD, relatórios, cópia do PDF, XFDF | já recusavam um caminho fora de teste (Fases 7 e 8) | iguais: o caminho vem do diálogo; um destino dentro de uma pasta da biblioteca é recusado |
| `e2e_*`, `bench_*` | já recusavam fora de teste ou de benchmark | iguais |
| `open_in_browser` | já só abria `http` e `https`, conferidos de novo no host | igual |
| `open_flow`, `flow_annotations`, … | por número de documento da biblioteca | iguais |

E a biblioteca não aceita mais uma pasta de livros que contenha os próprios arquivos dela (o banco, o
cache, as cópias de segurança), ou que esteja dentro deles: o app gravaria entre os livros que só pode ler.

### 3.4 Dependências, vulnerabilidades e licenças

- **RustSec** (`cargo deny check advisories`, com a base baixada em 2026-11-24): três avisos, todos
  corrigidos por versões de patch. `url` 2.5.8 (RUSTSEC-2026-0592: `Url::origin()` de um `blob:` aninhado
  estoura a pilha; o leitor usa `blob:` e o Tauri chama `origin()` sobre URLs da página) foi a 2.5.9;
  `libz-sys` 1.1.31 (RUSTSEC-2026-0544, no `gzwrite`, que o Musaeum não usa; a libarchive usa a zlib) foi a
  1.1.33; `cookie` 0.18.2 (RUSTSEC-2026-0594, só em cookies privados, que o Musaeum não usa) foi a 0.18.3.
  Depois: nenhum aviso.
- **npm** (`npm audit`): nenhuma vulnerabilidade, nos 17 pacotes que vão na página nem nos 96 só de
  desenvolvimento.
- **Licenças** (`cargo deny check licenses bans sources` e `tools/check-licenses.mjs`): todas na lista.
- **O que não passa por um gerenciador de pacotes** (PDFium chromium/8125, Tesseract 5.5.4, Leptonica
  1.87.0, DjVuLibre 3.5.30, libarchive 3.8.11, foliate-js) não tem uma base de avisos que se consulte
  offline. As versões estão fixadas pelo SHA-256, e todos rodam nos workers, nunca no processo principal.
- **`THIRD-PARTY-NOTICES.md`** cobre 25 componentes vendorizados, 334 crates e 17 pacotes npm, com cada
  texto de licença distinto uma vez (250 textos). Quinze pacotes publicam a licença só no manifesto, sem o
  arquivo (`tantivy-*`, `webview2-com*`, `selectors`, …); para eles vale o texto padrão da licença.

## 4. Decisões

### D1. O sandbox de cada worker é conferido de fora, nos testes e no app instalado

Até aqui, só o worker do PDF e o da ingestão tinham um teste que lia o token e as mitigações do processo
real, e só o `sandbox-probe` lia a própria tabela de handles. Agora o `musaeum-sandbox` tem o
`ProcessAudit`, que lê de fora, como o kernel o descreve, o token, as mitigações e cada handle de qualquer
processo que o host possa abrir, dá o nome do tipo de cada handle (também dos que o kernel não deixa
copiar, pela lista de tipos do Object Manager) e devolve uma lista do que fica aquém da política: uma
lista vazia é o único resultado que passa. Cada worker tem um teste que o inicia como o app o inicia e
exige a lista vazia, e o `tools/sandbox-audit` faz o mesmo nos workers vivos de um app em execução.

O que conta como falha: o token, o job ou as mitigações diferentes do que a política promete (inclusive o
win32k ligado num worker de `LockdownParser`, ou desligado num de `Lockdown`, que pediria rever a
política); um arquivo que o processo possa mudar; um arquivo que o host não deu (nos testes, onde se sabe
o que foi dado; no app em execução, qualquer arquivo só de leitura passa, porque os livros vêm e vão); uma
chave do registro gravável ou da hive do usuário; outro processo ou um token; uma seção com nome; a área de
trabalho da pessoa; mais de três pipes; e um handle de um desses tipos que não se consiga inspecionar.
Passam, com a razão escrita no código: os pipes do host, as pastas só de travessia (`FILE_TRAVERSE`) e os
dois dispositivos de criptografia do kernel, `\Device\CNG` (a biblioteca padrão do Rust, para números
aleatórios) e `\Device\KsecDD` (o `CryptGenRandom` da libarchive), que não guardam dado de ninguém.

A auditoria também lista as DLLs carregadas, e uma DLL de fora do Windows e da pasta do próprio worker é
uma falha: é assim que se vê código que outro programa pôs no processo, mesmo quando ele não segura
handle nenhum. Ela dá a idade do processo em cada leitura e, de um handle de outro processo, o executável
dono. As regras têm testes próprios, com um worker sintético para cada falha.

A auditoria não roda a cada início de um worker no app: um produto que injete algo em todo processo, como
alguns antivírus e o Windhawk desta máquina fazem (3.1), poderia fazê-la falhar numa máquina e deixar a
pessoa sem leitor. O app continua conferindo, a cada worker, que ele largou o token de partida, como
fazia.

Um worker que vive um pedido só (o das anotações do PDF) é acompanhado pelo `sandbox-audit --follow-ms`
durante o pedido inteiro. A fronteira é a do próprio sandbox: o lockdown, depois do qual o worker lê o
que o host manda. Antes de cada leitura, a ferramenta pergunta se alguma thread ainda segura o token de
partida (o lockdown não volta atrás, então uma leitura que começou depois dele só viu o que vem depois).
Uma leitura só conta como da partida com prova positiva: a thread com o token, ou o carregador do
Windows ainda sem ter rodado. Qualquer outra, as de resposta desconhecida inclusive, tem de passar sem
nenhuma falha: um erro no sinal só pode reprovar a mais, nunca aprovar. O que aparecer na partida não é
ignorado: vai ao relatório, e um handle de outro processo ou uma DLL de fora só passam se o dono for um
serviço em execução na sessão 0, fora do Windows e da instalação, e a DLL for da pasta do mesmo produto.

Um handle que o processo fecha entre a listagem e a cópia (o worker das anotações fecha o PDF ao
terminar) fica na lista como fechado e não conta: um handle que continua aberto é copiado na leitura
seguinte. Com os direitos de escrita é diferente: o que ele permitia se sabe pela listagem, e reprova
mesmo fechado.

### D2. O runtime do C e do C++ vai dentro de cada executável

Os workers importavam o `VCRUNTIME140.dll`, e os do OCR e do DjVu também o `MSVCP140.dll` e o
`VCRUNTIME140_1.dll`: numa máquina sem o Visual C++ Redistributable, eles nem começariam. O ADR 0004 tinha
deixado isso ao instalador. Em vez de levar o redistribuível (que pede administrador) ou cópias das DLLs ao
lado (que um worker de `LockdownParser` só carregaria se fossem aceitas como da Microsoft), o workspace
liga o runtime estaticamente (`-C target-feature=+crt-static` em `.cargo/config.toml`), e o `cc` compila o
Tesseract, o Leptonica, o DjVuLibre, a libarchive, o SQLite e a zlib com `/MT`. Os seis workers passaram
a importar só `kernel32`, `advapi32`, `ntdll`, `bcryptprimitives` e um conjunto de API do kernel: nem o
runtime do Visual C++, nem o Universal CRT. O app continua com o Universal CRT do Windows
(`api-ms-win-crt-*`, que leva ao `ucrtbase.dll`, parte do Windows 10 e 11), porque o `tauri-build` impõe
ao executável dele o "CRT híbrido" (o runtime do Visual C++ estático, o Universal CRT do sistema); isso não
pede nada que o Windows não tenha. O `pdfium.dll` já ligava o seu. O `stage-bundle.mjs` e o teste de
fumaça leem a tabela de importação de cada binário e recusam um que peça uma DLL que o Windows não tem.

### D3. A página só tem do Tauri o que usa

A capacidade da janela passou de `core:default` para `core:event:allow-listen` e
`core:event:allow-unlisten`: é tudo o que a página usa do próprio Tauri. Uma página comprometida não
consegue mais mudar a janela, abrir webviews, resolver caminhos ou ler o nome e a versão do app pelos
comandos do Tauri.

### D4. O host não abre nada só porque a página pediu

A regra que as exportações já seguiam (um caminho vem do diálogo do sistema, nunca da página, salvo sob
teste) vale agora para tudo o que lê:

- `open_session` só abre um arquivo que a biblioteca conhece (pelo caminho atual de um documento dela) ou
  o documento dado na linha de comando, e confere isso antes de ler um byte e antes de fechar o livro
  aberto. Sob `--e2e` ou `--bench`, abre o que for pedido, como antes.
- `library_add_root` só existe sob `--e2e`; uma pessoa escolhe a pasta no diálogo.
- `calibre_preview` e `calibre_import` só leem uma pasta que o diálogo devolveu nesta execução.
- O número de workers que a página pede para um documento fica entre 1 e 16.

As recusas dessas portas ficam em inglês, sem passar pelo catálogo: só uma página comprometida chega a elas
(ADR 0014, D3).

### D5. A biblioteca não acompanha os próprios arquivos

`Library::add_root` recusa uma pasta que contenha a pasta da biblioteca ou a do cache, ou que esteja dentro
delas: seguida, ela poria entre os livros o que o app grava o tempo todo (o banco, o cache, as cópias de
segurança), e a regra de nunca gravar numa pasta de livros deixaria de valer. A recusa vem no idioma da
pessoa (`RootHoldsLibrary`).

### D6. A versão 1.0.0 e os instaladores

- A versão é 1.0.0 no workspace do Cargo, no `package.json` (e no `package-lock.json`) e no
  `tauri.conf.json`, de onde o Tauri a grava no recurso de versão do executável, com o nome do produto, o
  editor (`Musaeum`) e o copyright. `musaeum --version` a diz.
- O **NSIS** instala só para o usuário (sem administrador), em português do Brasil ou em inglês, pelo
  idioma do Windows, sem perguntar. Os **MSIs** são dois, um por idioma, para a máquina toda; o código de
  atualização fica fixado (`038bf292-ffad-5c1b-985a-dbcbe8fd0452`), para que um MSI novo substitua o
  antigo mesmo que o nome do produto mude.
- O WebView2 não vai no instalador: numa máquina sem ele, o instalador baixa o instalador oficial. É o
  único acesso à rede, e só na instalação.
- A **linha de comando** não vai no instalador: é uma ferramenta de quem constrói do código, e o
  `musaeum.exe` dela teria o mesmo nome, para o Windows, que um `Musaeum.exe`.
- O executável continua `musaeum-desktop.exe`: os testes, os scripts e a documentação o chamam assim.
- O ícone é o mesmo, agora com todas as camadas que o Windows usa.

### D7. A configuração do instalador fica à parte

O `tauri-build` copia os `resources` para a pasta do build a cada compilação do app. Com os workers como
`resources` no `tauri.conf.json`, um `cargo test` copiaria os workers de release por cima dos de debug que
os testes usam. Por isso a configuração do instalador está em `tauri.bundle.json`, que só o
`npm run build:bundle` passa ao `tauri build` (`--config`), e os `resources` vêm de uma pasta de montagem
(`target/bundle-stage`), que o `stage-bundle.mjs` enche e confere.

### D8. Os avisos de terceiros são gerados e conferidos

`THIRD-PARTY-NOTICES.md` é escrito por `tools/third-party-notices.mjs` a partir do grafo de dependências
normais do app e dos workers no Windows (as de build e de teste não vão no programa), dos pacotes npm que
a página empacota e do `vendor/third-party.json`, com o texto de cada licença como o pacote o traz, uma vez
por texto distinto. A verificação de licenças falha se ele não estiver em dia, e o instalador o leva.

### D9. A assinatura está documentada, não feita

O build local não exige certificado. `docs/release.md` descreve como assinar uma publicação pública: os
workers e o `pdfium.dll` na pasta de montagem (o Tauri não assina `resources`), e o app, o desinstalador e
os instaladores pelo Tauri, com o certificado no repositório do usuário ou num HSM.

## 5. Resultados

**Gates**, no código final: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`,
`tsc --noEmit`, a conferência de licenças (cargo-deny, npm, o vendorizado e o `THIRD-PARTY-NOTICES.md` em
dia), os testes do verificador de licenças, `cargo deny check advisories` e `npm audit` passam. São 851
testes de Rust (eram 839 ao fim da Parte 1), todos passam e nenhum é ignorado; no Vitest, 232 passam e 1
é opcional, como antes. Os testes novos: as regras da auditoria (6, com um worker sintético para cada
falha), a auditoria de cada worker real (4, e a da ingestão estendida), a linha do lockdown vista de
fora e a recusa da pasta da biblioteca como pasta de livros; o `--version` da CLI entrou num teste que
já existia. A paridade dos dois idiomas está no Vitest
(`i18n.test.ts`) e no `musaeum-i18n`; a mensagem nova (`RootHoldsLibrary`) entrou nas duas línguas.

**Na janela real**, com o build final congelado, cada passada à espera da máquina livre:

| Passada | Resultado | Tempo |
|---|---|---|
| `e2e-preferences.mjs` | 23 de 23 | 18 s |
| `e2e-exchange.mjs` | 34 de 34 | 54 s |
| `e2e-pdf-annotations.mjs` | 24 de 24 | 49 s |
| `e2e-annotations.mjs --phase fixtures` | 30 conferências, todas passam | 99 s |
| `e2e-annotations.mjs --phase ocr` | 15 conferências, todas passam | 38 s |
| `e2e-beta.mjs --phase synthetic` | 24 de 24 | 17 s |
| `e2e-taxonomy.mjs` | 15 de 15 | 27 s |
| `e2e-ipc.mjs` (nova) | 36 de 36 | 7 s |
| `smoke-bundle.mjs` (nova) | 52 de 52 | 60 s |

**A passada do IPC**, com o app aberto sem `--e2e`: os seis comandos do Tauri tentados (`app|version`,
`app|name`, `window|title`, `webview|get_all_webviews`, `path|resolve_directory`, `resources|close`)
voltam "not allowed by ACL"; seguir uma pasta e ler ou trazer uma biblioteca do Calibre são recusados;
um PDF fora da biblioteca não abre, nem pelo caminho direto nem por `estante\..\fora\secreto.pdf`; um livro
da biblioteca abre, e 100.000 workers pedidos viram 16; as oito exportações e importações com caminho
dado pela página, os seis comandos de teste e de benchmark e quatro endereços que não são da web são
recusados; nada é gravado no disco, e nenhum worker fica depois de o app fechar.

**O teste de fumaça**: o NSIS instala em silêncio, por usuário, sem pedir administrador, numa pasta
temporária; os 13 arquivos estão lá, os modelos com o SHA-256 fixado, nenhum binário pede o runtime do
Visual C++ nem uma DLL que o Windows não tenha, e o recurso de versão diz Musaeum 1.0.0. O app instalado,
aberto só com as pastas do Windows no `PATH`, abre um quadrinho, um DjVu, um EPUB, um PDF e um escaneado,
lê por OCR uma caixa do escaneado, exporta JSON-LD, um relatório em PDF (lido de volta pelo PDFium) e uma
cópia do PDF com a anotação. Os seis workers foram vistos rodando da instalação, cada um sem nenhuma
violação; o das anotações, acompanhado durante o pedido, foi lido três vezes depois do lockdown, aos 55,
122 e 190 ms, sem nenhuma. O app fecha sem deixar processo nem WebView2. O desinstalador remove tudo menos
a si mesmo (que o teste apaga) e a chave com a pasta da instalação, que o teste confere e apaga. Cada MSI
tem as propriedades certas, o mesmo código de atualização e os mesmos arquivos; a imagem administrativa
de cada um tem os mesmos bytes que o NSIS instalou (menos o executável do app, que o Tauri marca com o tipo
do pacote) e abre um PDF. Ao fim, nada novo em `%TEMP%`, as pastas do Musaeum da pessoa como estavam,
nenhuma entrada de instalação.

O teste de fumaça só chegou a esse resultado depois de várias rodadas que acharam defeitos dele mesmo e
da auditoria
(seção 7, itens 11 e 12) e o Windhawk (3.1); a rodada final é a do build final.

**Os instaladores** (em `target\release\bundle\`, e cópias idênticas no build congelado das passadas):

| Arquivo | Bytes | SHA-256 |
|---|---|---|
| `nsis\Musaeum_1.0.0_x64-setup.exe` | 16.604.866 | `d5f787a0892f292376103a1dcbe9216d06b484d8e5b9145684bb446cd9efae14` |
| `msi\Musaeum_1.0.0_x64_pt-BR.msi` | 23.617.536 | `e5d8296d3f2bb75d1044025091c8f53e301b82a5076332c95d503e98de3903c3` |
| `msi\Musaeum_1.0.0_x64_en-US.msi` | 23.613.440 | `6dd2d5b9b5034619560be6338f409fcf71ec04a05d7c6be6e169f7b54028fc85` |

Os dois MSIs levam o código de atualização `{038BF292-FFAD-5C1B-985A-DBCBE8FD0452}`. Os hashes mudam a
cada build (seção 6).

**A assinatura**, exercida em cópias com um certificado descartável, num arquivo (sem tocar no
repositório de certificados do Windows): o `signtool sign /fd SHA256 /tr http://timestamp.digicert.com
/td SHA256` assinou um worker e o `pdfium.dll` com o carimbo de tempo da DigiCert; o `signtool verify /pa`
recusa só porque a raiz descartável não é confiável, como deve.

**O acervo**, só lido: 191 arquivos, 84 pastas, 8.877.205.812 bytes, com o SHA-256 de cada arquivo, o
tamanho, as datas de criação e modificação e os atributos de cada entrada, lidos antes da primeira cadeia
e depois da última: nenhuma diferença.

**Limpeza:** nenhum processo do Musaeum, das ferramentas ou do compilador do PowerShell; nada do Musaeum
nem dos instaladores em `%TEMP%` (há dois restos de um instalador NSIS de 17/11, de antes desta fase);
nenhuma chave nem atalho do Musaeum; as pastas da pessoa como estavam. A Lixeira tem cinco itens que a
própria pessoa apagou hoje, de pastas dela, antes das baterias; nenhum é de teste.

## 6. Limitações conhecidas

- **Os workers do PDFium alcançam o win32k** e carregam uma DLL que a Microsoft não assinou: o
  `pdfium.dll` pré-compilado importa o `user32`. É a maior superfície que sobra no sandbox (ADR 0001, D2).
  Uma build própria do PDFium, sem o `user32`, os levaria ao `LockdownParser`.
- **Um programa que injeta código em todo processo** (o Windhawk desta máquina, alguns antivírus) entra
  por alguns milissegundos nos workers do PDFium ao nascerem, às vezes até pouco depois do lockdown,
  quando o worker já pode estar lendo o livro (3.1). Os outros quatro workers o barram. Um serviço SYSTEM pode fazer qualquer coisa num
  processo do usuário, e nenhum sandbox de usuário o impede; o que o Musaeum controla é não lhe dar
  passagem, e isso só os workers de `LockdownParser` conseguem.
- **O que é vendorizado** (PDFium, Tesseract, Leptonica, DjVuLibre, libarchive) não tem uma base de
  avisos consultável offline: acompanhar os avisos de segurança dessas bibliotecas é trabalho de quem
  publica. Todos rodam só nos workers.
- **A auditoria de handles não roda a cada worker no app** (D1): roda nos testes, no teste de fumaça e
  quando se chama o `sandbox-audit`.
- **`--e2e` e `--bench` estão no binário de release** (3.2). Quem os usa já roda programas como o
  usuário.
- **Os instaladores desta versão não são assinados** (D9): o SmartScreen avisa quem os baixa.
- **Só o executável do app tem recurso de versão** (Musaeum 1.0.0, o editor, o copyright): os seis
  workers aparecem no Gerenciador de Tarefas só pelo nome do arquivo.
- **O instalador por usuário põe o programa em `%LOCALAPPDATA%\Musaeum`**, a mesma pasta em que a
  biblioteca guarda os dados, em subpastas próprias (`library`, `cache`) e no `preferences.db`. Os nomes
  não se cruzam, e desinstalar apaga só os arquivos que o instalador pôs (seção 5).
- **O build não é igual byte a byte** de uma vez para outra: as datas do linker, o código de pacote dos
  MSIs e as do NSIS mudam. O que se confere é o conteúdo.
- **O teste de fumaça roda nesta máquina**, que tem o WebView2 e o Visual C++ Redistributable instalados.
  Que o app não precisa do redistribuível é provado pela tabela de importação de cada binário, não por
  uma máquina sem ele; que o instalador baixa o WebView2 onde ele falta não foi exercido.

## 7. Defeitos achados no caminho

1. **Os workers não rodariam numa máquina sem o Visual C++ Redistributable.** Todos importavam o
   `VCRUNTIME140.dll`, e os do OCR e do DjVu também o `MSVCP140.dll` e o `VCRUNTIME140_1.dll`; o app, que o
   `tauri-build` já ligava em parte estaticamente, não. Nesta máquina o redistribuível está instalado, e
   nenhum teste o via. A leitura da tabela de importação achou (D2).
2. **A página podia abrir qualquer arquivo como documento.** `open_session` recebia um caminho e o abria,
   desenhava e lia o texto dele, fosse ou não da biblioteca: uma página comprometida leria qualquer PDF,
   quadrinho ou DjVu do disco (D4).
3. **A página podia mandar seguir qualquer pasta, e ler qualquer biblioteca do Calibre**, sem o diálogo do
   sistema (D4).
4. **A biblioteca aceitava como pasta de livros a pasta que contém os seus próprios arquivos**, e o app
   então gravaria entre os livros (D5). Dois testes da própria biblioteca faziam exatamente isso, por
   comodidade, e passaram a usar uma pasta de livros ao lado.
5. **A página tinha os comandos do próprio Tauri** (`core:default`) sem usar nenhum além dos eventos (D3).
6. **O servidor de desenvolvimento podia emoldurar um livro num build de release** (3.2).
7. **Três avisos do RustSec** nas dependências, um deles (`url`, `blob:` aninhado) no caminho da página
   (3.4).
8. **O ícone tinha só a camada de 256 px**, que o Windows reduzia na hora para 16 e 32.
9. **A CLI não dizia a sua versão.**

10. **A auditoria não olhava as DLLs carregadas**: código posto num worker por outro programa, sem
    handle, passaria. Ela as lista agora (D1); foi o Windhawk que mostrou a falta.
11. **Três falhas da própria verificação, achadas ao validá-la**: o `sandbox-audit` lia o token de
    partida depois da leitura inteira, quando o worker já podia ter feito o lockdown (um teste novo do
    worker do PDF prova o primitivo: o token está lá logo ao nascer e some no *Ready*, e a ferramenta
    agora pergunta antes de cada leitura); a passada do IPC montava o caminho com `..` por `join`, que o
    resolvia, e o caso de *path traversal* era igual ao primeiro (agora vai literal, e é recusado); e a
    auditoria contava como "não inspecionável" um handle fechado entre a listagem e a cópia.
12. **Três expectativas erradas do teste de fumaça**, achadas na primeira execução: o `/NS` do NSIS do
    Tauri tira os dois atalhos, não só o da área de trabalho; o worker das anotações do PDF vive um pedido
    só, e não estava vivo na hora da auditoria final; e a desinstalação silenciosa deixa
    `HKCU\Software\Musaeum\Musaeum` com a pasta da instalação (seção 5). A chave que aquela execução
    deixou foi conferida e apagada, e o teste agora a confere e apaga ele mesmo.

O `\Device\KsecDD` que o worker de quadrinhos segura não é um defeito: a auditoria nova o apontou, e ele
é o dispositivo de criptografia do kernel que o `CryptGenRandom` da libarchive abre (D1).

## 8. Pontos para a auditoria

Decisões em aberto de segurança ou de experiência antes da 1.0, que pedem a palavra do usuário:

1. **Os instaladores sem assinatura** (D9). A 1.0 pode sair assim, com o SHA-256 publicado, ou esperar um
   certificado; o procedimento está pronto e foi exercido com um certificado descartável (seção 5).
2. **`--e2e` e `--bench` no binário de release** (3.2). Tirá-los do build de release (uma feature do
   Cargo) fecharia a porta de vez, mas as passadas na janela passariam a testar outro binário que não o
   publicado. A proposta é mantê-los.
3. **A pasta do programa no instalador por usuário** é `%LOCALAPPDATA%\Musaeum`, a mesma da biblioteca
   (seção 6), porque é o padrão do Tauri para o nome do produto. Separá-los (`%LOCALAPPDATA%\Programs\
   Musaeum`, como fazem outros programas por usuário) pede um modelo próprio do NSIS. A proposta é manter
   para a 1.0, já que a desinstalação provadamente não toca nos dados.

4. **O Windhawk (ou outro injetor) nos workers do PDFium** (3.1). As mitigações do Windows avaliadas:

   | Mitigação | Impede a injeção? |
   |---|---|
   | Só DLLs da Microsoft desde a criação (`PROCESS_CREATION_MITIGATION_POLICY_BLOCK_NON_MICROSOFT_BINARIES_ALWAYS_ON`), como nos `LockdownParser` | Sim, e é por isso que os outros quatro workers nunca o têm; mas barra o próprio `pdfium.dll`, que não é da Microsoft |
   | A mesma política ligada depois de carregar o PDFium (`SetProcessMitigationPolicy`) | Não: a injeção acontece aos 2 a 5 ms, antes do `main` do worker |
   | Criar o worker por um caminho que o Windhawk não intercepta no app | Contornaria um programa que a própria pessoa instalou, e quebraria a cada versão dele; não recomendado |
   | O PDFium ligado ao executável do worker (a build própria do PDFium, ADR 0001, D2) | Sim: a política de só DLLs da Microsoft passa a valer para os dois workers do PDFium |
   | A lista de exclusão do próprio Windhawk (`musaeum-*.exe`) | Sim, do lado da pessoa |

   A proposta é publicar a 1.0 assim, com o achado documentado e a exclusão recomendada a quem usa um
   injetor, e levar a build própria do PDFium para a primeira versão depois da 1.0. A alternativa é segurar
   a 1.0 até essa build.

Escolhas que mudam o que vale como regra daqui em diante:

5. **O runtime do C e do C++ estático** em todos os executáveis (D2), no lugar do que o ADR 0004 previa
   (o instalador garantir o redistribuível).
6. **O host só abre o que a biblioteca conhece ou o que o diálogo deu** (D4): a página não dá caminhos,
   nem para ler. Um "abrir arquivo…" no futuro passa pelo diálogo do sistema.
7. **A biblioteca recusa uma pasta de livros que contenha os seus próprios arquivos** (D5).
8. **Das permissões do Tauri, a página só ouve eventos** (D3).
9. **A auditoria do sandbox roda nos testes e no teste de fumaça, não a cada worker** (D1); o
   `\Device\KsecDD` e o `\Device\CNG` passam, por não guardarem dado de ninguém.
10. **O que o instalador leva**: o app, os seis workers, o PDFium, os modelos `por` e `eng`, a licença e os
   avisos; não leva a linha de comando nem o WebView2; o NSIS por usuário em pt-BR e inglês pelo idioma do
   Windows; um MSI por idioma, para a máquina, com o código de atualização fixado (D6).
11. **O editor nos metadados é "Musaeum"** e o copyright, "Musaeum contributors", sem o nome de uma pessoa.

## 9. Como reproduzir

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps\desktop; npm test; npx tsc --noEmit; cd ..\..
node tools\check-licenses.mjs
cargo deny check advisories
cd apps\desktop; npm audit; npm run build:bundle; cd ..\..
cargo build --release -p musaeum-cli -p visual-check -p sandbox-audit -p gen-fixtures
node tools\e2e-ipc.mjs
node tools\smoke-bundle.mjs
```

## 10. A auditoria (2026-11-24)

Os onze pontos da seção 8 foram aceitos como estão implementados:

1. A 1.0.0 sai sem assinatura digital, com o SHA-256 de cada instalador publicado; o procedimento de
   assinatura fica em `docs/release.md` para as próximas versões.
2. `--e2e` e `--bench` ficam no binário publicado, para que as passadas na janela validem exatamente o
   executável distribuído.
3. O instalador por usuário mantém o programa em `%LOCALAPPDATA%\Musaeum`, dado que a desinstalação
   comprovadamente não apaga a biblioteca, o cache nem as preferências.
4. A 1.0.0 sai com o achado do Windhawk e a recomendação de excluir `musaeum-*.exe` num injetor
   documentados; a build própria do PDFium, ligada ao executável dos workers para levar os dois a
   `LockdownParser`, fica agendada para o ciclo depois da 1.0.
5. O runtime do C e do C++ fica estático em todos os executáveis.
6. O host só abre o que a biblioteca conhece ou o que o diálogo do sistema deu.
7. A biblioteca recusa uma pasta de livros que contenha os seus próprios arquivos.
8. Das permissões do Tauri, a página só ouve eventos.
9. A auditoria do sandbox roda nos testes e no teste de fumaça, não a cada worker no app.
10. O instalador leva o que a seção 8 lista, sem a CLI nem o WebView2, com o código de atualização fixado.
11. O editor nos metadados é "Musaeum" e o copyright, "Musaeum contributors".

Nenhuma dessas decisões muda código: os instaladores, os hashes e a bateria da seção 5 são os da versão
publicada.
