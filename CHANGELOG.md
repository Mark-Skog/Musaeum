# Histórico de versões

As mudanças de cada versão do Musaeum que chegam a quem o usa. As decisões e as medições de cada fase
estão em [`docs/adr/`](docs/adr/); o procedimento de uma versão, em [`docs/release.md`](docs/release.md).
O formato segue o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/), e as versões, o
[Versionamento Semântico](https://semver.org/lang/pt-BR/).

## 1.2.0 — 2026-11-28

Abas de múltiplos livros integradas à barra de título Win32, preservação instantânea da biblioteca
em memória e suspensão determinística de sessões WebGL2/workers em segundo plano.

### Adicionado

- **Barra de abas unificada estilo Adobe Acrobat (`40 px`):** botão rápido de Preferências (`⚙`),
  aba fixa Casa (`🏠`) conectada à biblioteca e faixa rolável horizontal de livros abertos com
  deduplicação automática, fechamento via `×` ou botão do meio, espaçador de arrasto garantido
  (`min-width: 56 px`), alinhamento exato de `WM_NCHITTEST` em 100%/125%/150%/200% DPI e título
  nativo dinâmico na barra de tarefas e no `Alt+Tab`.
- **Atalhos globais de abas (inclusive dentro de EPUB/MOBI/AZW3/FB2):** `Ctrl+Tab` /
  `Ctrl+Shift+Tab`, `Ctrl+W` / `Ctrl+F4` e `Ctrl+1`..`Ctrl+9` (`Ctrl+1` retorna à biblioteca),
  preservando combinações `AltGr` em teclados ABNT2.
- **Preservação da biblioteca em `0 ms`:** a grade virtualizada, a posição de rolagem, a busca, os
  filtros e o painel de Detalhes permanecem em memória ao alternar entre livros e `🏠`; o botão
  `← Biblioteca` alterna para a aba Casa mantendo o livro aberto.
- **Snapshots completos por aba (`ReaderTabSnapshot`):** preserva e reidrata sem salto visual a
  página, deslocamento exato de viewport, zoom, modo de ajuste, spreads, posição/CFI fluido,
  ferramenta ativa e filtros do painel de anotações, protegendo contra sobrescrita por padrões
  durante carregamentos assíncronos.
- **Suspensão determinística de GPU, SHM e workers (teto rígido de 1 sessão ativa):** abas em
  segundo plano liberam explicitamente recursos WebGL2 (`TileRenderer.destroy()`,
  `WEBGL_lose_context`), caches e workers; na aba Casa (`🏠`), o aplicativo mantém `0` sessões de
  renderização e `0` buffers `ICoreWebView2SharedBuffer` ativos.
- **Tratamento isolado de casos de borda:** detecção em disco de arquivos movidos ou excluídos ao
  reabrir uma aba suspensa (com erro localizado e ação para fechar apenas aquela aba), sincronização
  automática ao editar/mesclar/remover obras na biblioteca e persistência segura da aba ativa ao
  fechar a janela (`CloseGuard`).

### Corrigido

- **Controles `VisuallyHidden` dentro de `ScrollFrame`:** ancoragem CSS (`position: relative`) nos
  controles React Aria e nos contêineres roláveis internos (`.prefs-body`, `.inspector-body`, etc.),
  garantindo que os inputs ocultos de acessibilidade acompanhem a rolagem 1:1 com seus rótulos.

## 1.1.0 — 2026-11-25

Polimento visual e ergonômico da biblioteca, barra de título nativa customizada para Windows 10/11 e
correção crítica de estabilidade na abertura concorrente de PDFs grandes.

### Adicionado

- **Barra de título customizada nativa (Windows 10/11):** integrada aos temas claro, escuro e sistema
  via subclassing Win32 (`WM_NCCALCSIZE`, `DwmExtendFrameIntoClientArea` com compensação por DPI ao
  maximizar), preservando Aero Snap, redimensionamento pelas bordas, `Win + Setas`, `Win + Z`,
  ocultação em tela cheia (`F11` inclusive dentro de EPUBs) e controles localizados em `pt-BR` e `en`.
- **Scrollbar de overlay com amortecimento exponencial:** sem setas, sem deslocamento lateral (*zero
  layout shift*), fade-in/fade-out exponencial por proximidade de borda ou rolagem e suavização
  exponencial tanto no arrasto do polegar quanto na rodinha do mouse (`wheel`), respeitando o modo de
  movimento reduzido.
- **Barra lateral redimensionável e persistente (`180–420 px`):** alça divisória com movimento
  exponencial sem queda de FPS em bibliotecas grandes, duplo clique para restaurar `240 px`, atalho
  `Ctrl+B` com animação suave, ocultação automática de seções vazias (com botão `+ Organizar`) e
  omissão visual de pasta raiz única quando possui subpastas.
- **Seleção múltipla de pastas** no diálogo nativo do Windows (`FOS_ALLOWMULTISELECT`) e diálogo
  dedicado para gerenciar pastas acompanhadas.
- **Tamanho fixo das capas e zoom na biblioteca:** as capas mantêm dimensão constante durante o
  redimensionamento da janela ou dos painéis; novo ajuste de tamanho via `Ctrl + Scroll` sobre a
  grade e em Preferências (`120–260 px`, padrão `168 px`).
- **Preferências adicionais na barra lateral:** opções para exibir/ocultar contagens de livros
  (desligada por padrão) e seções vazias.

### Melhorado

- **Barra superior da biblioteca enxuta:** ações secundárias consolidadas em um menu compacto (`⚙`),
  alternador vetorial `Capas`/`Lista` e destaque visual persistente nos livros selecionados ou abertos
  em Detalhes.
- **Transições exponenciais de layout:** abertura e fechamento suaves do painel lateral direito
  (`Detalhes` / edição em lote) e reposicionamento fluido dos cards da grade durante mudanças de
  painel, sem interferir na virtualização durante a rolagem.

### Corrigido

- **Encerramento abrupto (`STATUS_ACCESS_VIOLATION` / `0xc0000005`) ao abrir PDFs grandes:** eliminado
  *use-after-unmap* de `ICoreWebView2SharedBuffer` durante aberturas ou fechamentos concorrentes de
  sessão, substituindo mapeamento externo emprestado por duplicação de `HANDLE` de seção NT com view
  própria no `ShmRegion`, escopo de buffers por sessão e serialização do ciclo de vida tanto no leitor
  de páginas fixas quanto no leitor fluido.

## 1.0.0 — 2026-11-24

A primeira versão estável, para Windows 10 e 11 (x64), com instalador.

### Adicionado

- **Instaladores.** Um instalador NSIS por usuário, sem pedir administrador, em português do Brasil ou
  em inglês, e um MSI para a máquina toda em cada uma das duas línguas, para quem administra várias
  máquinas. Levam o app, os seis workers, o PDFium, os modelos `por` e `eng` do OCR, a licença e os
  avisos de terceiros (`THIRD-PARTY-NOTICES.md`). Desinstalar não apaga a biblioteca, as anotações nem
  as preferências.
- **Preferências** (Ctrl+,): o idioma da interface (português do Brasil ou inglês), o tema (o do
  sistema, claro ou escuro, também na barra de título), o movimento reduzido, o nome do autor das
  anotações, os idiomas do OCR entre os modelos verificados e a limpeza do cache. Valem na hora e ficam
  num `preferences.db` fora das pastas de livros.
- **Dois idiomas, por inteiro**: toda a interface e tudo o que o programa escreve para a pessoa, as
  recusas, os títulos dos diálogos, o assunto das anotações num PDF ou XFDF e os relatórios de estudo.
- **Acessibilidade**: contraste WCAG AA nos dois temas, também nas bordas dos campos e no anel de foco;
  diálogos e menus que prendem o foco e o devolvem; painéis que fecham com Esc; nomes em todo botão de
  símbolo; o trabalho em segundo plano anunciado aos leitores de tela.
- **Anotações em JSON-LD do W3C**, de um livro, de várias obras ou da biblioteca, sem perder nada, e de
  volta sem duplicar, também numa outra edição do mesmo livro; e **relatórios de estudo** em Markdown e
  em PDF, na ordem de leitura e pelos capítulos.
- **As anotações do próprio PDF**: as do Musaeum levadas a uma cópia do arquivo como anotações que o
  Acrobat e o PDFium mostram, as de um PDF trazidas para a biblioteca, e as duas coisas em XFDF.
- `musaeum --version` na linha de comando.

### Segurança

- Cada worker (PDF, anotações do PDF, quadrinhos, DjVu, OCR e ingestão) é conferido de fora, nos testes
  e no app instalado (`tools/sandbox-audit`): token restrito de integridade baixa, mitigações do
  processo, job object, cada handle que ele segura e cada DLL que carregou. Nenhum segura arquivo
  gravável, chave do registro do usuário, outro processo ou a área de trabalho da pessoa, nem código de
  fora do Windows e do Musaeum. (Um programa que injeta código em todo processo, como alguns antivírus,
  entra por milissegundos nos workers do PDFium ao nascerem; os outros quatro o barram.)
- A página do app passa a poder só ouvir os eventos do app entre os comandos do próprio Tauri (antes,
  `core:default`).
- O host não abre mais um documento que não seja da biblioteca ou o da linha de comando, não segue uma
  pasta nem lê uma biblioteca do Calibre que o diálogo do sistema não tenha dado, e limita a 16 os
  workers de um documento, mesmo que a página peça mais. Conferido por `tools/e2e-ipc.mjs`, com o app
  aberto como uma pessoa o abre.
- A biblioteca recusa acompanhar uma pasta que contenha os seus próprios arquivos, ou que esteja dentro
  deles.
- A política de segurança da página ganhou `object-src`, `base-uri` e `form-action`; a do leitor fluido
  não deixa mais o servidor de desenvolvimento emoldurar um livro num build de release.
- Dependências atualizadas por avisos de segurança: `url` 2.5.9, `libz-sys` 1.1.33 e `cookie` 0.18.3.

### Mudado

- O runtime do C e do C++ vai dentro de cada executável: o app instalado não precisa do Visual C++
  Redistributable.
- O ícone do app tem todas as resoluções do Windows (16 a 256 px).

### Corrigido

- A busca no texto não achava o que o OCR tinha lido num livro escaneado: o índice nunca recebia essas
  linhas. Os escaneados que já tinham linhas lidas são indexados de novo, uma vez.
- Num PDF, uma marca sobre texto impresso girado ficava com o topo embaixo: o PDFium dá o ângulo dos
  caracteres no sentido horário. As anotações de livros de layout fixo são ancoradas de novo, uma vez, ao
  abrir.

## 0.2.0-beta — 2026-11-23

O marco Beta: a biblioteca organizada e muitos livros de uma vez.

### Adicionado

- **Organização**: categorias em árvore (até 16 níveis), etiquetas, coleções feitas à mão e coleções
  inteligentes por regra, ao lado das pastas do disco.
- **Detalhes**: o painel que edita a obra (título, pessoas e papéis, série, ano, idioma,
  identificadores com o ISBN conferido, avaliação, estado de leitura, sinopse e campos personalizados) e
  diz de onde veio cada campo; nada lido de um arquivo escreve por cima do que a pessoa editou.
- **Vários livros de uma vez**: Ctrl+clique, Shift+clique e Ctrl+A, e a edição de todos numa transação.
- **Revisar**: os arquivos que podem ser o mesmo e as obras que podem ser um só livro, e a fusão sem
  perder anotações, posição, histórico, categorias nem etiquetas.
- **Busca no texto dos livros**, em segundo plano: o trecho achado, a página ou a seção, e o livro aberto
  ali.
- **Importação do Calibre**, só lendo a biblioteca dele, com as colunas personalizadas.
- **Livros escaneados**: as linhas de texto achadas na imagem da página (retas, curvas, giradas a 45° e
  90°), as ferramentas Linhas, Caixa e Traço, e o texto sob cada marca lido por OCR (Tesseract, num
  processo isolado), para copiar e buscar.
- **Anotações**: realces em cinco cores, sublinhados e notas em Markdown, em livros fixos e fluidos,
  ancoradas de três jeitos e achadas de novo depois de renomear, mover ou editar o arquivo, com o
  Inspetor de Anotações e busca sem acentos.

## 0.1.0-alpha — 2026-11-10

O marco Alpha: uma biblioteca de uso diário com o leitor integrado.

### Adicionado

- **Biblioteca** que segue as pastas escolhidas, sem nunca escrever nelas: capas ou lista, busca
  instantânea nos títulos, filtro por pasta, e a posição de cada livro guardada exata.
- **Leitor fluido** para EPUB 2 e 3, MOBI, AZW3, FB2, TXT e Markdown, normalizado num processo isolado:
  páginas com a virada animada ou rolagem contínua, fonte, tamanho, entrelinha, margens e tema; cada
  livro numa origem própria, sem acesso ao app, e sem scripts.
- **Leitor de layout fixo** para PDF, quadrinhos (CBZ, CBR) e DjVu, desenhado na escala exata da tela
  por workers isolados, com páginas duplas, zoom nítido no quadro seguinte e recuperação de um worker que
  cai.
- **Linha de comando** (`musaeum`): pastas, reconciliação, busca, inspeção, revisões, mover e desfazer,
  e `watch`.
- Por baixo: SQLite com identidade de arquivo, indexador e watcher que avisa quando perde eventos, diário
  de operações com desfazer, e os processos isolados que leem bytes hostis.
