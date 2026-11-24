# ADR 0014: decisões da Fase 8, Parte 1, as preferências, os dois idiomas, o design e a acessibilidade

| | |
|---|---|
| **Data** | 2026-11-24 |
| **Situação** | Aceita na auditoria de 2026-11-24, sem emendas (seção 9) |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo, WebView2 157 |
| **Escopo** | Uma superfície de Preferências fora do acervo (idioma, tema, movimento, autor das anotações, idiomas do OCR, diagnóstico e limpeza do cache); a interface inteira e tudo o que o lado Rust escreve para a pessoa em português do Brasil e em inglês, com testes de paridade e de strings esquecidas; tokens de design com contraste WCAG AA; teclado, foco e ARIA; a recusa de gravar dentro das pastas da biblioteca estendida às ações da Parte 1 da Fase 7; testes e a passada na janela real |

## 1. Resumo

- **Preferências**, pelo botão da biblioteca, pelo ⚙ do leitor ou por **Ctrl+,** de qualquer lugar. Tudo
  vale na hora: o idioma (pt-BR ou inglês), o tema (sistema, claro, escuro, também na barra de título), o
  movimento (o do sistema ou reduzido), o nome do autor das anotações, os idiomas do OCR entre os modelos
  verificados, e o cache (quanto ocupa, e esquecer as linhas e o OCR). Ficam em `preferences.db`, ao lado do
  banco da biblioteca e nunca numa pasta de livros, criado só na primeira mudança.
- **Dois idiomas, por inteiro.** As 577 chaves da interface estão em 11 áreas de catálogo, em
  `pt-BR/` e `en/`; as 298 mensagens do lado Rust estão num catálogo só, com as duas línguas lado a lado.
  Recusas e erros, títulos dos diálogos do sistema, nomes sugeridos, o `/Subj` das anotações num PDF e
  num XFDF, os rótulos, as contagens e as datas dos relatórios de estudo: tudo no idioma escolhido, na
  hora. Testes garantem as mesmas chaves e os mesmos valores nos dois idiomas, nenhuma chave sem uso, e
  nenhuma palavra escrita fora dos catálogos nas superfícies da interface.
- **Design.** As cores, a tipografia, os espaços, os cantos, as sombras, o anel de foco e o movimento são
  tokens. Todos os pares de texto e fundo passam de 4,5:1 nos dois temas; a borda dos controles, o anel
  de foco e os estados mostrados só por cor passam de 3:1. Um teste lê as cores do próprio `app.css`.
- **Teclado e leitores de tela.** Diálogos, menus e popovers prendem o foco, fecham com Esc e o devolvem.
  Os painéis ao lado não prendem o foco, mas o devolvem a quem os abriu quando fecham, e Esc neles faz o
  que se espera. Todo botão que mostra só um símbolo tem nome, conferido por teste. O que acontece em
  segundo plano (exportar, indexar, ler por OCR) é anunciado por regiões `aria-live` e `aria-busy`.

## 2. Entregas

| Onde | O quê |
|---|---|
| `crates/musaeum-i18n/` | Crate novo: `Locale`, o idioma atual do processo, o catálogo (`catalog.rs`, uma macro que exige as duas línguas), `text`, `count`, `tr!`, `tr_in!`; testes de paridade de valores, de inglês sem letras do português, e de nenhuma mensagem sem uso no workspace |
| `crates/musaeum-library/src/preferences.rs`, `migrations/preferences/` | O `preferences.db` (`application_id` "MUSP"): carregar, mudar tudo ou nada, guardar, validar |
| `crates/musaeum-library/src/cache_report.rs` | O tamanho de cada parte do cache, e esquecer as linhas e o OCR sem tocar no resto |
| `crates/musaeum-ocr/src/models.rs` | Os modelos do Tesseract conhecidos, com o SHA-256 de cada um; só os verificados são oferecidos e lidos |
| `crates/musaeum-report/` | O relatório no seu idioma (`Report::locale`) e com o autor quando há um (`Report::annotator`) |
| `crates/musaeum-library/src/*.rs` | As recusas que uma pessoa ou um arquivo dela provoca, no catálogo: taxonomia, metadados, coleções inteligentes, lote, fusão, Calibre, pastas, diário de operações, anotações, o JSON-LD trazido, e o `Display` dos erros de abrir a biblioteca |
| `apps/desktop/src-tauri/src/preferences.rs` | Os comandos (`preferences_get`, `preferences_set`, `ocr_models`, `cache_report`, `cache_clear_lines`), o idioma do lado Rust, a barra de título |
| `apps/desktop/src-tauri/src/*.rs` | As mensagens do host no catálogo; o autor em `/T` e no XFDF; a recusa de gravar a cópia do PDF e o XFDF numa pasta da biblioteca |
| `apps/desktop/src/app/i18n/` | Os catálogos da interface, `t`, `tn`, `useT`, `useLocale`, os formatos de número, tamanho e data |
| `apps/desktop/src/app/preferences/` | O diálogo e o estado das preferências na página (`data-theme`, `data-motion`, o idioma) |
| `apps/desktop/src/app/focus.ts` | O foco devolvido quando um painel ao lado fecha, e Esc nos painéis |
| `apps/desktop/src/app/app.css` | Os tokens de design, a borda dos controles, as transições, a animação dos diálogos |
| `apps/desktop/checks/` | `strings.test.mjs` (nenhuma palavra fora dos catálogos, nenhuma chave sem uso, nenhum botão sem nome) e `contrast.test.mjs` (WCAG AA nos dois temas) |
| Leitores | O movimento reduzido ao vivo nos dois leitores; os títulos das seções do leitor contínuo no idioma atual |
| `tools/e2e-preferences.mjs` | A passada na janela real |
| `tools/fetch-ocr.ps1` | `-Languages spa,fra…`, com os hashes fixados |

## 3. Decisões

### D1. As preferências moram fora do acervo

`preferences.db` é um SQLite próprio (`application_id` "MUSP", `journal_mode=DELETE`, `synchronous=FULL`),
com uma linha por preferência, a chave e o valor em JSON. Fica em `%LOCALAPPDATA%\Musaeum\` ou, quando o
app recebe `--library DIR`, em `DIR\preferences.db`, para que cada teste tenha as suas. Nunca fica numa
pasta de livros. O arquivo só é criado na primeira mudança: abrir o app não escreve nada.

Uma mudança é validada inteira e gravada numa transação: ou muda tudo o que foi pedido, ou nada. Um valor
que não se consegue ler volta ao padrão sem apagar o que está gravado, e uma chave desconhecida (de uma
versão futura) fica onde está. Um banco que não é do Musaeum é recusado.

### D2. Os catálogos: duas línguas lado a lado, e o compilador cobra

Na interface, cada área tem um arquivo em `pt-BR/` e o seu par em `en/`, tipado por ele
(`Record<keyof typeof pt, string>`): uma chave que falta ou sobra em inglês não compila. As chaves
começam pelo nome da área. No Rust, a macro `catalog!` gera o `enum T` e o texto de cada mensagem nas duas
línguas, e uma mensagem sem uma delas não compila.

Os valores vão entre chaves com nome (`{count}`), e os dois lados conferem que as duas línguas usam os
mesmos nomes. Uma contagem escolhe entre duas mensagens, `.one` para exatamente um e `.other` para o resto,
o que basta para as duas línguas (ver a seção 5). O idioma muda na hora: a página redesenha o que mostra
(`useSyncExternalStore`), e o host troca o idioma do processo, que vale para a próxima mensagem escrita.

O português que já existia continua o mesmo, palavra por palavra, de modo que os testes que procuram
textos na janela continuam valendo. As recusas da biblioteca, que antes só existiam em inglês, ganharam
o português.

### D3. O que é escrito para uma pessoa, e o que não é

Vai para o catálogo tudo o que uma ação da pessoa ou um arquivo dela pode provocar: uma recusa, um erro de
gravar, o título de um diálogo, o nome de um tipo de arquivo, o nome sugerido, o que é escrito dentro de
um arquivo para outro programa mostrar (o `/Subj` e o rótulo da coleção de JSON-LD), e os rótulos dos
relatórios. Fica em inglês, fora do catálogo, o que só aparece se o próprio programa estiver
inconsistente ou numa corrida (o documento que já não está aberto, uma invariante de um seletor, o
protocolo com um worker), os registros (`eprintln!`) e o painel de diagnóstico da página de benchmark das
Fases 0 a 2, que não é parte do app. Quando uma dessas aparece, vem depois de uma frase no idioma da
pessoa que diz o que não pôde ser feito.

A CLI (`musaeum`) fala inglês, no uso e nas mensagens, e por isso fixa o inglês ao começar: as recusas da
biblioteca que ela mostra saem em inglês também. O idioma é uma preferência do app.

O que é gravado fica no idioma em que foi escrito: o erro de uma operação no diário, o "Sem título" de um
livro do Calibre sem título, o rótulo da coleção de um JSON-LD exportado. O `/Subj` é lido de volta em
qualquer dos dois idiomas.

### D4. O relatório tem o seu idioma

`Report` ganhou `locale` (fora do JSON do relatório) e `annotator`. Cada rótulo, contagem, data e nome de
cor sai do catálogo naquele idioma: em português, `23/11/2026 às 17:40`; em inglês, `2026-11-23 at 17:40`,
sem ambiguidade de dia e mês. O mesmo relatório no mesmo idioma dá os mesmos bytes; o teste de ouro do
Markdown em português ficou igual, byte por byte, e um teste novo confere o inglês. O que a pessoa
escreveu e o que o livro diz não são traduzidos.

### D5. O autor das anotações

O nome das Preferências (até 128 caracteres, sem caracteres de controle) vai para o `/T` de cada anotação
na cópia do PDF e para o `title` de cada uma no XFDF. Vazio, os dois levam o nome de usuário do Windows,
como antes. O relatório só nomeia alguém quando a pessoa deu um nome: uma linha "Anotações de …" e o
campo `annotator` do front matter. Uma anotação que veio de outro programa com o seu próprio autor
continua com ele.

### D6. Os idiomas do OCR: só modelos verificados

`musaeum-ocr` tem uma tabela dos modelos `tessdata_fast` conhecidos, com o SHA-256 de cada um: `por` e
`eng` vêm com o programa; `spa`, `fra`, `deu`, `ita` e `lat` são opcionais. O host lê os bytes, confere o
hash e só então os entrega ao worker no sandbox, que continua sem abrir arquivo nenhum. Um arquivo com
outros bytes não é oferecido nem lido. A pessoa escolhe de um a quatro idiomas entre os verificados;
`tools\fetch-ocr.ps1 -Languages spa,fra` baixa os opcionais, com os mesmos hashes, e recusa um código que
não conhece. Não foi preciso crate novo: o `sha2` já estava no `Cargo.lock`.

### D7. O cache: ver e esquecer sem perder nada

O diálogo mostra quantas páginas têm linhas guardadas e quantas linhas o OCR leu, com o tamanho, e o
tamanho do índice de texto, dos livros fluidos normalizados, das capas e do `cache.db`. **Esquecer as
linhas e o OCR** pede confirmação (com o botão de cancelar focado) e apaga só aquelas linhas do
`cache.db`, que depois é compactado (`VACUUM` e o checkpoint do WAL). O `library.db`, onde ficam as
anotações, não é tocado, e o índice de texto guarda o que já tinha. O que foi esquecido é refeito quando
a página é aberta de novo.

### D8. Tema e movimento

O tema é `data-theme` na raiz, com três estados (sem atributo, o do sistema; `light`; `dark`), e a barra de
título segue a escolha pelo `set_theme` do Tauri, ao abrir e a cada mudança. O movimento é `data-motion`:
`reduce` quando a pessoa pediu ou o sistema pede (`prefers-reduced-motion`), e então uma regra zera todas
as transições e animações. Os dois leitores recebem a mudança ao vivo: as páginas viram sem animação, sem
que a escolha de cada livro, guardada com a posição, seja apagada.

### D9. Os tokens de design e o contraste

O `:root` declara as cores aos pares (claro e escuro), a escala de tipos (11, 12, 13, 14, 16, 20 e 32 px),
os espaços (4 a 24 px), os cantos (2 a 12 px, e a pílula), as sombras (`--shadow`, `--shadow-raised`), o anel de foco e as
durações. Todo tamanho de fonte em pixels e todo canto de `app.css` agora é um token (um círculo continua
`50%`); dois tamanhos que fugiam da escala foram levados ao degrau mais próximo (10 px a 11 px, 15 px a
16 px).

Os pares de texto já passavam de 4,5:1. O que faltava era o critério 1.4.11: a borda de um campo de texto,
de uma busca e de uma estrela vazia usava `--line` (1,2:1 a 1,5:1). Ela ganhou um token próprio,
`--control-line` (3,3:1 ou mais no claro, 3,5:1 ou mais no escuro), e `--line` ficou para as linhas que só
separam. A estrela marcada ganhou `--star` (mais de 3,4:1 no claro, 10:1 no escuro). `checks/contrast.test.mjs` lê as cores do próprio
`app.css`, confere que os dois blocos escuros dizem o mesmo e que toda cor tem o seu par, e mede cada par.

### D10. Teclado e foco

Os diálogos, menus e popovers são os do React Aria, que prendem o foco, fecham com Esc e o devolvem a quem
os abriu. Os painéis ao lado não são modais: o foco entra e sai deles livremente. Quando um deles fecha com
o foco dentro, o foco voltaria ao `body`; agora volta ao botão que o abriu (o sumário e as anotações do
leitor) ou aos livros (os detalhes). Esc fecha o sumário e as anotações; nos detalhes, que podem ter uma
edição por salvar, Esc volta aos livros, e um segundo Esc os fecha, como já fechava quando o foco estava nos
livros. **Ctrl+,** abre as Preferências de qualquer lugar e está em `aria-keyshortcuts`.

### D11. A recusa de gravar dentro da biblioteca, também na Parte 1 da Fase 7

A cópia do PDF com anotações e o XFDF agora passam pela mesma verificação do JSON-LD e dos relatórios: um
destino dentro de uma pasta que a biblioteca acompanha é recusado antes de qualquer escrita, e o diálogo de
salvar não abre mais na pasta do livro.

## 4. Resultados

**Gates.** `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, a conferência de
licenças e `tsc --noEmit` passam. São 839 testes de Rust (eram 824 ao fim da Fase 7), todos passam, e
nenhum é ignorado. No Vitest, 232 passam, entre eles:

- os 8 de `i18n.test.ts`: as mesmas chaves e os mesmos valores nas duas línguas, as duas formas de cada
  contagem, nenhum português no inglês, nenhuma chave em duas áreas, e as mensagens com os seus valores,
  números e tamanhos;
- os 3 de `checks/strings.test.mjs`: nenhuma palavra fora dos catálogos, nenhuma chave sem uso, nenhum
  botão sem nome;
- os 6 de `checks/contrast.test.mjs`.

No Rust, `musaeum-i18n` confere os valores das 298 mensagens, o inglês sem letras do português, e que
nenhuma mensagem fica sem uso: um teste à parte, com uma mensagem falsa posta de propósito, mostrou que ele
a acha.

**Na janela real** (`tools/e2e-preferences.mjs`), com o build congelado: 23 de 23 passos, em 18 s.

- O padrão é pt-BR, o tema e o movimento do sistema, sem autor, OCR em `por` e `eng`, e nenhum arquivo
  escrito até a primeira mudança. Depois, o `preferences.db` está ao lado da biblioteca e não na estante.
- No teclado: Tab e Shift+Tab, 55 vezes, ficam nas Preferências; Esc as fecha, e o foco volta ao botão.
  Ctrl+, as abre com o foco dentro. Depois de salvar o autor e depois de esquecer o cache, o foco continua
  no diálogo, e o Esc ainda o fecha. Nos detalhes, Esc volta aos livros e o segundo Esc os fecha, com o
  foco nos livros. O sumário do leitor fecha com Esc e devolve o foco ao seu botão.
- O tema escuro pinta a página de `rgb(24, 25, 27)` e o claro de `rgb(246, 245, 242)`, na hora. O do
  sistema não põe `data-theme`.
- O movimento reduzido leva as transições de `0.12s` a `0s`. No leitor fixo paginado, as páginas param de
  animar (`animating: false`) e a escolha do livro fica (`animationPreference: true`), e voltam a animar
  quando o movimento é o do sistema.
- O inglês vale na hora: o diálogo, a barra, a grade dos livros e o leitor. O português volta na hora.
- O que o lado Rust escreve, em inglês e depois em português:
  - a recusa de gravar na estante ("is a folder of the library", "é uma pasta da biblioteca"), sem nada
    escrito lá;
  - na cópia do PDF, `/T` = "Ana Lima" e `/Subj` = "Highlight", depois "Realce", lidos pelo PDFium;
  - no XFDF, `title="Ana Lima"` e `subject="Highlight"`, depois "Realce";
  - o relatório em Markdown, com "Study report made on 2026-…", "Annotations by Ana Lima.",
    "**Highlight** · yellow" e `annotator: "Ana Lima"`, depois "Relatório de estudo gerado em …",
    "Anotações de Ana Lima." e "**Realce** · amarelo";
  - o relatório em PDF, lido pelo PDFium, com "page 1 of" e depois "página 1 de".
- Esquecer o cache pede confirmação, com o botão de cancelar focado. Todas as anotações ficam, e o
  `cache.db` vai de 160 KB a 74 KB. O livro da passada tem texto, então o cache não tinha linhas nem OCR
  para esquecer: que eles são esquecidos, e que a citação lida pelo OCR fica, quem prova é o teste
  `clearing_the_lines_and_the_ocr_forgets_them_and_keeps_every_annotation`.
- Fechado e aberto de novo: as preferências guardadas (inglês, escuro, movimento reduzido, o autor, OCR
  só em `eng`), e a página em inglês, escura e parada desde a primeira pintura; o teclado no diálogo de
  novo, em inglês.

**Regressões na janela**, com o mesmo build, todas passam:

| Passada | Resultado | Tempo |
|---|---|---|
| `e2e-exchange.mjs` (Fase 7, Parte 2) | 34 de 34 | 53 s |
| `e2e-pdf-annotations.mjs` (Fase 7, Parte 1, com o passo da recusa dentro da biblioteca) | 24 de 24 | 49 s |
| `e2e-annotations.mjs --phase fixtures` (Fases 4 e 5) | todas as conferências | 101 s |
| `e2e-annotations.mjs --phase ocr` (Fase 5) | todas as conferências | 39 s |
| `e2e-beta.mjs --phase synthetic` (Fase 6) | 24 de 24 | 19 s |
| `e2e-taxonomy.mjs` (Fase 6) | 15 de 15 | 28 s |

**O acervo**, só lido, tem 191 arquivos, 84 pastas e 8.877.205.812 bytes antes e depois. Cada arquivo e
cada pasta têm o mesmo tamanho, a mesma data e os mesmos atributos.

**Limpeza:**

- Ao fim, nenhum processo do Musaeum está rodando.
- A Lixeira está vazia, como antes.
- Nada `musaeum*` ficou em `%TEMP%`. As cinco entradas novas lá são do atualizador do Edge, do atualizador
  do Office e um `.tmp` vazio de antes da primeira passada.
- Um processo órfão de um teste da CLI foi encerrado, e a sua pasta temporária apagada (seção 7, item 4).

**O caminho.** A primeira cadeia achou o Esc engolido (seção 7, item 1) e dois roteiros. A segunda, e uma
passada das Preferências à parte, acharam o tempo do roteiro novo e o foco que saía das Preferências
(item 2). Cada correção foi seguida de um build
novo, congelado, e da cadeia inteira de novo; os números acima são os da terceira cadeia, completa e sem
falhas.

## 5. Limitações conhecidas

- **Duas línguas, duas formas de plural.** `.one` e `.other` bastam para o português e o inglês. Uma
  língua com mais formas pediria `Intl.PluralRules` na página e o equivalente no Rust.
- **O que é gravado não muda de idioma** (D3): o erro guardado no diário, o "Sem título" do Calibre, o
  rótulo de um JSON-LD já exportado.
- **Mensagens internas em inglês** (D3), atrás de uma frase no idioma da pessoa.
- **A barra de título** segue o tema, mas a passada na janela não a confere: o protocolo de depuração da
  WebView2 não enxerga a área fora da página.
- **O verificador de strings** acha frases, palavras com maiúscula, com acento ou com reticências; uma
  palavra única, minúscula e sem acento escrita fora do catálogo passaria. As superfícies foram revistas à
  mão também.
- **O nome de usuário do Windows** continua indo para o `/T` quando nenhum autor foi escolhido (D5).
- **A passada na janela esquece um cache sem linhas nem OCR** (o livro dela tem texto); o efeito sobre
  eles é coberto pelo teste de Rust da seção 4, não pela janela.
- **O painel de benchmark** das Fases 0 a 2 continua em português: ele não é parte do app (D3).

## 6. Como reproduzir

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps\desktop; npm test; npx tsc --noEmit; cd ..\..
node tools\check-licenses.mjs
npm --prefix apps\desktop run build:app
node tools\e2e-preferences.mjs
```

## 7. Defeitos achados no caminho

1. **Um segundo Esc rápido era engolido.** As Preferências e os diálogos ganharam uma entrada suave, e os
   popovers também. O React Aria mantém na página um popover que está fechando até a sua animação
   acabar, e um popover recém-aberto ainda estava animando: o primeiro Esc fechava a lista, o segundo
   caía nela, já de saída, e o diálogo por baixo nunca o recebia. A passada da taxonomia achou isso ("o
   diálogo não fechou"), e um diagnóstico na janela confirmou: com 400 ms entre as teclas, fechava. Os
   popovers voltaram a aparecer na hora, e só a entrada dos diálogos é animada; a saída, que nunca era
   usada, saiu também.
2. **O foco saía das Preferências** depois de esquecer o cache: a confirmação, com o foco dentro, sumia,
   o foco caía no `body`, e o Esc não fechava mais o diálogo (a passada nova achou isso, e o clique seguinte
   batia no fundo do diálogo). O mesmo acontecia ao salvar o autor pelo botão, que se desliga assim que o
   nome está guardado. O foco agora volta ao botão de esquecer, ou ao campo do nome, e a passada confere
   as duas coisas.
3. **A CLI mostrava as recusas da biblioteca em português** no meio de uma saída em inglês, e dois testes
   dela falharam. Ela agora fixa o inglês ao começar (D3).
4. **Um teste da CLI deixava um processo órfão quando falhava.** O `musaeum watch` que ele abre só era
   encerrado no fim do teste; com a asserção falhando antes, ficou rodando com a sua biblioteca
   temporária, e ainda segurou o arquivo de log dos gates, de modo que os últimos passos não apareceram.
   O processo foi encerrado e a pasta apagada, com a autorização do usuário, e o teste agora encerra o
   filho num `Drop`, falhe ou não.
5. **A chave `taxonomy.scheme.other` parecia uma forma de plural** (`.other`) e quebrava o teste das
   contagens. Virou `taxonomy.scheme.otherScheme`.
6. **A borda dos campos e a estrela vazia ficavam abaixo de 3:1**, e a estrela marcada a 2,4:1 (D9).
7. **Dois botões sem nome:** "1" e "2" das páginas lado a lado. Ganharam "Uma página por vez" e "Duas
   páginas lado a lado".
8. **Uma chave sem uso** (`prefs.shortcut`), achada pelo teste novo, foi removida: o atalho já está em
   `aria-keyshortcuts`.
9. **Os roteiros:** a passada nova lia as preferências fora da vista que o gancho devolve, e clicava antes
   de os campos dos detalhes e o botão do cache chegarem; a passada do PDF gravava uma segunda cópia em
   `exportados`, que ela mesma passa a acompanhar ao abrir a primeira cópia, e agora é recusada, como
   deve (D11). A segunda cópia vai para outra pasta.

## 8. Pontos para a auditoria

Escolhas que mudam o que a pessoa vê ou que valem como regra daqui em diante:

1. **A data em inglês é ISO** (`2026-11-23 at 17:40`), não a americana, para não confundir dia e mês
   (D4).
2. **A CLI fala inglês sempre**, sem seguir as Preferências do app (D3).
3. **A regra do que fica em inglês** (D3): as mensagens que só aparecem com o programa inconsistente, os
   registros e o painel de benchmark. E o que foi gravado fica no idioma em que foi escrito.
4. **O relatório só nomeia o autor quando ele foi escolhido**; o PDF e o XFDF continuam levando o nome de
   usuário do Windows quando não foi (D5).
5. **As bordas dos campos ficaram mais escuras** (`--control-line`) para chegar a 3:1, e dois tamanhos de
   fonte foram levados ao degrau mais próximo (10 a 11 px, 15 a 16 px) (D9).
6. **Só os diálogos entram com animação**; os popovers e os menus aparecem na hora, e nada anima ao sair
   (seção 7, item 1).
7. **Esc nos detalhes volta aos livros, e só o segundo os fecha**, para que uma edição por salvar não se
   perca num Esc (D10).
8. **Os idiomas opcionais do OCR são baixados pelo script**, não pelo app, que continua sem acessar a
   rede; o app só os oferece depois de conferir o hash (D6).
9. **A barra de título segue o tema**, mas a passada na janela não a enxerga (seção 5).
10. **Na janela, o cache esquecido não tinha linhas nem OCR**; o efeito sobre eles é provado pelo teste de
    Rust (seção 4).

## 9. A auditoria (2026-11-24)

Os dez pontos da seção 8 foram aceitos como estão implementados:

1. A data em inglês é ISO, `2026-11-23 at 17:40`.
2. A CLI fala inglês sempre e fixa o inglês ao começar.
3. Ficam em inglês as mensagens internas, os registros e o painel de benchmark, e o que foi gravado fica no
   idioma em que foi escrito.
4. O relatório só nomeia o autor quando ele foi escolhido nas Preferências.
5. As bordas dos controles têm 3:1 (`--control-line`), e a escala de tipos ficou com os degraus ajustados.
6. Só os diálogos entram com animação; popovers e menus aparecem na hora, e nada anima ao sair.
7. Esc nos detalhes volta aos livros, e o segundo Esc os fecha.
8. Os idiomas opcionais do OCR são baixados pelo `tools\fetch-ocr.ps1`, não pelo app, que continua sem
   acessar a rede, e só são oferecidos depois de conferido o hash.
9. A barra de título segue o tema, e a passada na janela não a confere: é uma limitação conhecida.
10. O efeito de esquecer o cache sobre as linhas e o OCR fica provado pelo teste de Rust, não pela janela.

Os pontos 1 a 5, 7 e 9 foram aprovados um a um. Os pontos 6, 8 e 10 foram aprovados com a regra de que
seriam registrados como aceitos se nenhum deles pedisse uma decisão aberta de experiência ou de segurança
antes da 1.0. Nenhum pede: o 6 é uma escolha visual já medida na janela (seção 7, item 1), o 8 mantém o
app sem rede, e o 10 é uma questão de onde a prova está, e ela existe.
