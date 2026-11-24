# Histórico de versões

As mudanças de cada versão do Musaeum que chegam a quem o usa. As decisões e as medições de cada fase
estão em [`docs/adr/`](docs/adr/); o procedimento de uma versão, em [`docs/release.md`](docs/release.md).
O formato segue o [Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/), e as versões, o
[Versionamento Semântico](https://semver.org/lang/pt-BR/).

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

