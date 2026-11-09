# ADR 0005: decisões da Fase 3, Parte 1

| | |
|---|---|
| **Data** | 2026-11-04 |
| **Situação** | Decisões aceitas. O projeto auditou e aprovou a Parte 1, com as decisões da seção 8. Os dois testes na janela real (seções 5.4 e 5.5) rodaram em 2026-11-09, com o computador livre, e passaram |
| **Máquina das medições** | A mesma das fases anteriores: Ryzen 5 5600G, 31 GB, RX 7600, monitor de 75 Hz, SSD NVMe, Windows 11, Microsoft Defender ativo |
| **Escopo** | O normalizador dos formatos fluidos (EPUB 2 e 3, MOBI, AZW3, FB2, TXT, Markdown) no processo isolado `ingest`, e o motor de posição (EPUB CFI e Locator). O adaptador de leitura e a rolagem contínua ficam para a Parte 2 |

## 1. Resumo

Todo livro fluido passa a ter uma forma só: um pacote EPUB 3 gerado pelo Musaeum, com documentos XHTML bem formados
e inertes, cortados em partes de no máximo 200 KiB, e um `musaeum.json` que descreve tudo. Quem faz isso é o
crate novo `musaeum-flow`, que roda no worker `ingest`, no sandbox mais estrito. O Core recebe o pacote pelo pipe,
confere a forma e o conteúdo de novo e o guarda no cache pelo hash do arquivo e pela versão do normalizador.

Os resultados que mais pesam:

1. **Os 13 EPUBs do acervo, sem uma falha.** Normalizados duas vezes cada no worker isolado, com bytes idênticos nas
   duas, nenhuma reinicialização do worker e nenhuma falha do Core. O acervo continuou intacto (seção 5.1).
2. **2.135.148 posições, ida e volta exata.** Em todos os documentos dos 13 livros, cada nó de texto (no início, no
   meio e no fim) e cada elemento virou CFI e Locator, passou por JSON e voltou ao mesmo ponto, ao mesmo Locator e ao
   mesmo CFI do leitor (seção 5.2).
3. **O CFI do Musaeum é o CFI do EPUB.** Para EPUB, o CFI canônico conta nos documentos do livro, não nas partes.
   Escrito sobre o DOM normalizado, ele leva ao mesmo texto no documento original em 640.807 de 640.807 posições: é o
   CFI que qualquer outro leitor escreveria (D6).
4. **E o leitor concorda.** No Chrome headless, o `epubcfi.js` do foliate-js, o mesmo do leitor, achou o texto certo
   para 75.180 de 75.180 CFIs dos livros reais e 102.990 de 102.990 das fixtures e dos arquivos do Calibre, e
   reescreveu cada um deles idêntico (seção 5.3).
5. **Nenhuma parte acima de 200 KiB.** Em 7 dos 13 livros, 47 documentos de até 785 KiB foram cortados entre blocos;
   a maior parte do acervo tem 203.235 bytes, abaixo dos 204.800. Nenhum parágrafo, tabela, bloco de código ou figura
   foi partido (D4).
6. **Os seis formatos dão o mesmo livro.** As fixtures escrevem um mesmo livro em EPUB 3, EPUB 2, MOBI 6, KF8, FB2,
   TXT e Markdown, e cada um sai com o mesmo texto, os mesmos capítulos no sumário e os mesmos metadados. Arquivos
   feitos pelo Calibre, um escritor independente, abrem igual (D9).
7. **O livro hostil sai sem nada que execute ou chame a rede.** Os 39 vetores do EPUB hostil da Fase 0 saem
   neutralizados, e um verificador escrito à parte do sanitizador não acha nada em nenhuma parte (D5). Na janela real,
   sem CSP e sem sandbox nos iframes, o livro normalizado não rodou um script e não chegou à rede (seção 5.4).
8. **Relayout abaixo de 100 ms no livro que antes levava 1,1 s.** No `large.epub` normalizado, o pior P95 de todas as
   mudanças foi 70 ms, e a posição ficou em todas as 360 medições; sem o corte, as seções de 3 MiB levavam até 1.100 ms
   (seção 5.5).

## 2. Entregas

| Entrega | Como foi testado | Resultado |
|---|---|---|
| Ajustes da auditoria da Fase 2 | Teste da regra de 95%; o binário de `tests/cloud.rs` 100 vezes seguidas | Feitos; ADR 0004, seção 8 |
| Commit e consolidação da Fase 2 | `5615a68` na `fase-2`, fast-forward na `main`, `fase-2` apagada, `fase-3` criada | Feito |
| Normalizador no worker `ingest` | Fixtures dos seis formatos e o livro hostil, no sandbox `LockdownParser`; 19 arquivos de teste e os 13 do acervo | Seções 5.1 a 5.4 |
| Divisão das seções longas | Capítulo de 1,15 MiB nas fixtures (EPUB e TXT), `large.epub` da Fase 0 (1 e 3 MiB), 47 documentos reais | Nenhuma parte acima do orçamento |
| Sanitização | Livro hostil de 39 vetores, verificador independente em cada parte, de novo no host | Seção 5.4 |
| TOC, metadados, estilos, recursos | Fixtures de cada formato; 13 livros reais | 1.861 entradas de sumário reais, todas com CFI; 13 capas; 9.481 recursos |
| CFI e Locator | Ida e volta em todas as posições; contra o DOM original; no navegador com foliate-js | Seções 5.2 e 5.3 |
| Cache por hash e versão | `FlowStore`, gravação atômica, releitura idêntica | Teste no sandbox |
| CLI | `musaeum normalize LIVRO SAÍDA` | Usada nos testes de janela |

**Testes.** 576 em Rust no workspace (eram 498) e os mesmos 125 em TypeScript. Todos passam, com `clippy -D warnings`,
`fmt` e `tsc --noEmit` limpos e as licenças conferidas, inclusive as das dependências novas. Os 78 testes novos cobrem o
DOM, os dois parsers, o serializador, o CSS, o sanitizador, o corte, o CFI, o Locator, cada formato, o protocolo e o
worker no sandbox real.

## 3. Como foi medido

- **No worker de verdade.** Os livros são normalizados pelo `musaeum-ingest.exe` no sandbox `LockdownParser` (sem
  win32k, só DLLs da Microsoft), recebendo o arquivo como handle somente leitura, como o app fará. O resultado volta
  pelo pipe e o host o confere antes de guardar (D2).
- **Acervo.** `tools/corpus-check --flow-only` lista o acervo inteiro antes e depois, normaliza cada livro fluido duas
  vezes, guarda o resultado no cache do diretório de trabalho e confere cada posição. O cache e o relatório ficam
  fora do acervo. Para comparar com o DOM original dos EPUBs, a ferramenta lê os livros no próprio processo: é uma
  ferramenta de teste, e o app nunca faz isso.
- **Navegador.** `tools/cfi-browser-check.mjs` serve cada parte como `application/xhtml+xml` em `127.0.0.1` para um
  Chrome headless com perfil temporário (sem janela, nada do usuário) e resolve os CFIs com o `epubcfi.js` do
  foliate-js. É o mesmo motor do WebView2.
- **Calibre.** Para ter um escritor independente do meu, o `ebook-convert` do Calibre 9.16 converteu dois livros do
  Projeto Gutenberg para MOBI 6, MOBI conjunto (MOBI 6 + KF8), AZW3, FB2 e TXT, na pasta temporária.
- **Concorrência.** O usuário usava o computador (VS Code, Roblox Studio aberto) enquanto o acervo e os testes sem
  janela rodavam, abaixo da prioridade normal. Contagens, posições e bytes não mudam com isso; os tempos de
  normalização da seção 5.1 são piores que numa máquina parada. Os testes na janela real (seção 5.4 e 5.5) só
  começaram com o computador livre.

## 4. Decisões

### D1. Um crate novo, `musaeum-flow`, todo em Rust seguro

O normalizador e o motor de posição ficam juntos, num crate com `#![forbid(unsafe_code)]`, sem E/S. O worker o usa
para normalizar; o Core, para as posições. Dependências novas, todas já permitidas pela política de licenças:
`html5ever` 0.39 (já estava na árvore, pelo Tauri), `cssparser` 0.37 (idem), `encoding_rs`, `chardetng`,
`pulldown-cmark` e `sha1_smol` (só para a chave da ofuscação de fontes do IDPF).

O DOM é próprio: só elementos e texto, com texto adjacente sempre fundido. Comentários e instruções de
processamento nunca entram, e CDATA vira texto. É exatamente o que um CFI enxerga, então o DOM em que o CFI é
calculado não tem nada que o navegador conte de outro jeito.

### D2. O worker normaliza; o host confere de novo

O protocolo passa à versão 2: o pedido ganha `task` (`Extract` ou `Normalize`), e a resposta `Normalized` diz
quantos arquivos e quantos bytes vêm a seguir, crus, cada um com caminho e tamanho. O host lê com limites
conferidos antes de alocar (100 mil arquivos, 512 MiB por arquivo, 1 GiB no total) e só aceita caminhos relativos,
de até três níveis, com letras, dígitos, `.`, `_` e `-`. Depois:

- o `musaeum.json` tem de existir, ser da versão do normalizador e citar só arquivos que vieram;
- cada parte XHTML ou SVG é lida de novo com o parser XML estrito e passa pelo verificador independente de
  perigo. Um worker tomado por um livro que devolvesse um script seria recusado aqui.

O livro é lido no worker por posição (`seek_read`), não do ponteiro do arquivo: o handle duplicado divide o ponteiro
com o do host, e um segundo pedido sobre o mesmo arquivo lia zero bytes. O corpus-check achou isso ao normalizar
cada livro duas vezes.

Uma bomba ZIP (um capítulo que descomprime para 2 GiB) para no limite de 256 MiB por arquivo, e o capítulo vira
um documento substituto com aviso; o resto do livro abre, e o worker segue de pé.

### D3. XML estrito primeiro, HTML5 se precisar

Um XHTML é lido como XML (quick-xml, com escopo de namespaces próprio). Duas tolerâncias: as entidades nomeadas do
HTML valem em qualquer documento (como num navegador com doctype XHTML), e prefixos conhecidos (`epub:`, `xlink:`)
funcionam sem declaração. Nada externo é buscado: DTDs e entidades externas são ignoradas, e as internas são texto
com limite de expansão. Profundidade máxima de 1.024 e 2 milhões de nós por documento.

O que não é XML bem formado vai para o html5ever, o algoritmo dos navegadores, com scripts desligados (o conteúdo de
`<noscript>` vira marcação, porque é ele que um leitor sem scripts deve mostrar). `text/html`, MOBI e o que o
Markdown gera vão direto para ele. O resultado é sempre escrito como XHTML bem formado, com namespaces declarados
onde precisa e os caracteres que o XML não aceita fora. Nos 13 livros do acervo, todos os documentos eram XML
bem formado.

### D4. Corte entre blocos, com o caminho de início de cada parte

Um documento cujo XHTML passa de 200 KiB (o limite da D5 do ADR 0001) é cortado. O corte só cai antes de um elemento
de bloco: nunca dentro de parágrafo, título, tabela, `pre`, figura, item de lista ou trecho de texto corrido. Um
contêiner grande demais (`div`, `section`, `blockquote`, listas) é aberto, e se repete em cada parte em volta da sua
fatia, com os atributos, para os estilos continuarem valendo; uma `ol` que continua ganha `start`. As partes saem
equilibradas: o capítulo de 1,15 MiB das fixtures vira seis partes de 195 a 196 KiB, e a seção de 3 MiB do
`large.epub`, dezesseis de 188 a 193 KiB, em vez de partes cheias e uma sobra pequena.

Cada parte guarda onde começa no documento inteiro, como passos de CFI abaixo de `<body>`. Só com isso dá para levar
uma posição do documento para a parte e de volta por aritmética, sem abrir nenhuma das duas, e juntar as partes no
documento original (testado byte a byte). Um bloco sozinho maior que o orçamento, como uma tabela enorme, fica
inteiro numa parte acima do limite e é contado como `oversizeParts`; no acervo, nenhuma.

O efeito no layout está na seção 5.5.

### D5. Neutralizar sem tirar do lugar

Em `<body>`, o que pode executar ou embutir algo vivo não é removido: vira um `span` ou `div` inerte no mesmo lugar,
marcado com `data-musaeum-removed`. Assim os elementos seguintes mantêm a posição, e um CFI do livro original
continua apontando o mesmo nó. Em `<head>`, onde posição não importa, essas coisas saem.

| O quê | Como fica |
|---|---|
| `script` (HTML e SVG) | Vazio e oculto; em `<head>`, removido |
| `iframe`, `frame`, `frameset`, `portal` | Inerte e oculto, filhos mantidos (para o CFI) |
| `object`, `embed`, `applet` | Inerte; o conteúdo alternativo continua visível |
| `form` | `div`; `action` e `formaction` saem dos controles |
| `meta http-equiv`, `base`, `link` que não seja folha de estilo | Removidos de `<head>` |
| Atributos `on*`, `srcdoc`, `srcset`, `ping`, `target`, `xml:base` e afins | Removidos |
| SVG `set` e `animate*` que miram `href` ou eventos | `g` vazio e oculto |
| `template` | Sem conteúdo, como no DOM de um navegador |

Toda URL passa por um resolvedor que conhece o livro: o que aponta para dentro é reescrito para o pacote
normalizado; imagens e fontes de fora, `javascript:`, `data:text/html`, `file:` e afins são removidos. Links para
`http(s)`, `mailto` e `tel` ficam como links: só são seguidos com um clique, que o leitor intercepta. **Aprovado na
auditoria (seção 8):** o pedido era remover referências externas; removi tudo que carrega sozinho e mantive
hiperlinks, que são conteúdo (bibliografias, notas).

O CSS passa pelo tokenizador do Servo (`cssparser`): escapes como `u\72l(…)` não escondem nada. Cada `url()`, cada
string dentro de `image-set()` e cada `@import` vai para o resolvedor; `expression()` e `-moz-binding` saem;
`@charset` sai (a saída é UTF-8); comentários viram `/**/`. O mesmo vale para `style="…"` e para atributos de
apresentação do SVG com `url(…)`. Imagens SVG soltas também são sanitizadas.

Um verificador escrito à parte do sanitizador (`find_danger`) confere cada parte depois, nos testes, no corpus-check e
no host (D2), para um não esconder o erro do outro.

### D6. O CFI canônico conta nos documentos do livro

Há dois CFIs para a mesma posição:

- o **canônico**, guardado no Locator, conta nos documentos do livro: `/6/N` é o N-ésimo item do spine da fonte (com
  o `id` do `itemref` como asserção), e o caminho é o do documento inteiro;
- o **do leitor** conta nas partes do pacote normalizado, que é o que o foliate-js vê.

A conversão entre os dois é exata e não abre nenhum documento (D4). Por que assim: o canônico não muda quando o
corte muda, e, como a neutralização não tira nada do lugar, para EPUB ele é o mesmo CFI do livro original (seção
5.2). Isso deixa a importação e a exportação de anotações de outros leitores, na Fase 7, sem tradução.

Para MOBI, AZW3, FB2, TXT e Markdown, os documentos são os que o conversor produz, e o CFI vale para a versão do
normalizador que o escreveu, como o plano previa. Os documentos sem `itemref` de origem não levam asserção no passo
do spine.

Os deslocamentos contam unidades UTF-16, como as strings do JavaScript; uma posição no meio de um par substituto não
existe. Um CFI com intervalo (`,`), deslocamento espacial (`@`) ou temporal (`~`) é recusado: não é posição de
leitura. Asserções de `id` vencem a contagem, como manda a especificação, e a correção é informada.

### D7. O Locator

No formato do Readium, em JSON:

```json
{
  "href": "OEBPS/Text/chapter03.xhtml",
  "type": "application/xhtml+xml",
  "title": "Capítulo 3",
  "locations": { "cfi": "epubcfi(/6/8[ch3]!/4/2/22/1:15)", "progression": 0.4231, "totalProgression": 0.1234 },
  "text": { "before": "…32 caracteres…", "highlight": "…32…", "after": "…32…" },
  "normalizer": 1
}
```

`href` é o documento da fonte. `progression` e `totalProgression` vêm do texto, em unidades UTF-16, sem relógio nem
layout: são determinísticos e voltam iguais. A citação é tirada do documento inteiro, então atravessa o lugar onde
ele foi cortado. Para achar um Locator: o CFI, se a versão do normalizador é a mesma; depois a citação, a ocorrência
mais próxima da progressão, contando ocorrências sobrepostas (sem isso, 12 posições em blocos de código do acervo
iam para outra ocorrência); por fim a progressão.

### D8. Formato por formato

| Formato | O que vira documento | Sumário | Observações |
|---|---|---|---|
| EPUB 2 e 3 | Cada `itemref`, na ordem, mesmo quebrado (vira um substituto, para o passo do spine não mudar); depois os XHTML fora do spine | `nav` (toc, page-list, landmarks), senão NCX; `guide` como landmarks | Fallbacks seguidos; imagem no spine vira página; ofuscação IDPF e Adobe desfeita; qualquer outra criptografia é DRM e o livro é recusado; arquivos esquecidos no manifesto entram se forem imagem, fonte ou CSS |
| MOBI 6 | Trechos entre `<mbp:pagebreak/>` | Índice NCX (INDX), senão títulos | PalmDOC e HUFF/CDIC; âncoras inseridas nos bytes que os `filepos` apontam, fora de tags, entidades e caracteres; `recindex` para imagens |
| KF8 (AZW3, e a metade KF8 de um MOBI conjunto) | Cada esqueleto com seus fragmentos | Índice NCX por `fid` e deslocamento | Fluxos do FDST viram CSS; `kindle:embed`, `kindle:flow` e `kindle:pos` resolvidos; fontes `FONT` desmascaradas e descomprimidas; `aid` vira `id` só onde um link aponta |
| FB2 (e `.fb2.zip`) | Cada seção do primeiro nível; o que vem antes; cada `body` de notas | Títulos das seções | Codificação da declaração (Windows-1251 incluído); poemas, epígrafes e tabelas com classes; imagens base64; notas ligadas entre documentos |
| TXT | Cada capítulo | Títulos reconhecidos | Codificação adivinhada (BOM, UTF-8, chardetng); parágrafos por linha em branco, ou um por linha; título só com palavra-chave e número ("Capítulo 3", "CHAPTER XII"); três ou mais seguidos são um sumário, não capítulos |
| Markdown | Cada título de nível 1, se houver vários | Títulos 1 a 3 | CommonMark com tabelas, notas, tachado e tarefas; front matter (título, autor, língua); HTML cru sanitizado; imagens só como `data:` |

Os links de formatos que escrevem tudo como fragmento de um livro só (FB2, Markdown, MOBI) são reapontados para o
documento que tem o `id`. HUFF/CDIC foi testado só com tabelas sintéticas: nem o Calibre nem as fixtures o escrevem.

### D9. Fixtures de um livro só, em todos os formatos

`gen-fixtures --only flow` escreve um livro-modelo em EPUB 3 (com o capítulo 3 de 1,15 MiB, tabelas, listas, código,
figuras, MathML, SVG, fonte ofuscada, notas em outro documento, CSS com `@import` e URLs de fora), EPUB 2 (NCX,
`opf:role`, entidades do doctype XHTML, um capítulo que não é XML bem formado, uma imagem no spine), MOBI 6 (PalmDOC,
`filepos`, guia, NCX), KF8 (esqueletos, fragmentos, FDST, `kindle:pos` no meio de um fragmento, fonte XOR + zlib),
FB2 em UTF-8 e em Windows-1251, TXT em Windows-1252 com quebras CRLF e um sumário, e Markdown com front matter,
tabela, código, nota, imagem `data:` e um script. Cada parágrafo começa com uma marca (`[c3p17]`), e os testes
procuram o mesmo texto em todos. Os escritores de MOBI e KF8 são meus, como o leitor; os arquivos do Calibre são a
conferência independente.

### D10. Cache pelo hash e pela versão

`cache/flow/v1/ab/<BLAKE3>/` guarda o pacote. É gravado inteiro numa pasta ao lado e renomeado de uma vez; um leitor
acha um livro completo ou nenhum. Uma versão nova do normalizador grava ao lado, em `v2`, em vez de ler o que não
escreveu.

### D11. Metadados de MOBI, AZW3 e FB2 no indexador

Com os formatos novos no protocolo, o indexador passa a ler os metadados de MOBI, AZW3 e FB2 só pelos cabeçalhos
(EXTH, `<description>`), sem descomprimir texto. `EXTRACTOR_VERSION` sobe para 3, então o que foi lido antes será
lido de novo.

## 5. Métricas

### 5.1 Acervo real (somente leitura)

| | |
|---|---|
| Acervo | 157 arquivos, 71 pastas, 7.822.677.893 bytes: tamanho, datas e atributos idênticos antes e depois. Os 13 EPUBs somam 271 MB |
| Normalização | 13 de 13 livros, cada um duas vezes, com bytes idênticos nas duas. 0 reinicializações do worker, 0 falhas do Core |
| Documentos | 827 documentos em 887 partes; 47 documentos cortados, em 7 livros; o maior documento de origem tem 785 KiB, a maior parte 203.235 bytes; 0 partes acima do orçamento |
| Leitura | Todos os documentos eram XML bem formado (nenhum foi para o HTML5). 0 avisos |
| Navegação | 1.861 entradas de sumário, todas com CFI; 3.566 entradas de lista de páginas; 65 marcos |
| Metadados | 13 títulos, 13 capas, 13 línguas (4 `pt`, 8 `en`, 1 `eng`, como os livros escrevem) |
| Arquivos | 9.481 recursos, 310 MB (imagens, fontes, CSS sanitizado); 204 arquivos deixados de fora (NCX, páginas de modelo da Adobe, tipos desconhecidos) |
| Sanitização | 40 `script` neutralizados, todos do Real-Time Rendering; 535 diretivas de `<head>` (`meta http-equiv` de codificação, quase todas); 4 URLs removidas; 549 links para a web mantidos |
| Tempo, máquina em uso | Ida e volta no worker, com a transferência: de 44 ms a 1,68 s por livro, mediana de 238 ms. O mais lento é o Real-Time Rendering (4.835 arquivos) |
| Verificação inteira | 47 s para os 13 livros, normalizando duas vezes e conferindo cada posição |

### 5.2 Posições

Cada nó de texto foi testado em três pontos (início, meio e fim) e cada elemento como ponto; cada posição virou Locator, foi
para JSON e voltou, e o CFI canônico foi para o CFI do leitor e voltou.

| | Acervo (13 EPUBs) | Fixtures e Calibre (19 arquivos, 6 formatos) |
|---|---:|---:|
| Posições | 2.135.148 | 555.203 |
| De volta ao mesmo ponto, ao mesmo Locator e ao mesmo CFI do leitor | 2.135.148 | 555.203 |
| CFI canônico que leva ao mesmo texto no documento original (só EPUB) | 640.807 de 640.807 | 122.834 de 122.834 |
| Achadas de novo só pela citação, com o CFI inutilizado | 16.391 de 16.391 | 4.160 de 4.160 |

A comparação com o original inclui os documentos com scripts neutralizados (o livro hostil, o Real-Time Rendering): é
para isso que a neutralização não tira nada do lugar. A primeira rodada achou 12 citações em outra ocorrência, em blocos
de código, onde o mesmo texto se repete sobreposto; corrigido (D7), deu 16.391 de 16.391.

### 5.3 No navegador

| | CFIs | Mesmo texto no Chrome | Reescritos idênticos pelo foliate-js |
|---|---:|---:|---:|
| 13 livros do acervo | 75.180 | 75.180 | 75.180 |
| Fixtures, livro hostil, `large.epub` e arquivos do Calibre | 102.990 | 102.990 | 102.990 |

Até 40 nós de texto por parte, cada um no início, a um terço e no fim. "Reescrito idêntico" é o `fromRange` do foliate-js
sobre o range que ele mesmo resolveu, comparado com o CFI escrito em Rust, asserções de `id` incluídas. O Chrome
parseia cada parte como `application/xhtml+xml`; nenhuma deu erro de XML.

### 5.4 Livro hostil

Sem janela, o livro hostil da Fase 0 (39 vetores: 19 formas de rodar script, 20 de sair para a rede sem script) sai com
9 scripts neutralizados, 5 atributos de evento removidos, 8 frames e objetos neutralizados, 5 diretivas de `<head>`, 2
formulários e 14 URLs removidas. O verificador independente não acha nada em nenhuma das 40 partes nem nas duas imagens
SVG, e nenhum arquivo do pacote cita o servidor canário.

**Na janela real** (2026-11-09, computador livre e sem input do começo ao fim, WebView2 157). O teste abre o livro
hostil normalizado no app, com o servidor canário escutando, primeiro sem nenhuma outra barreira (sem CSP e com
`allow-scripts allow-same-origin` nos iframes das seções) e depois com as duas barreiras:

| Livro | Barreiras | Seções | Capítulos carregados | Scripts que rodaram | Chegaram ao canário | Rede | Mensagens ao app |
|---|---|---:|---:|---:|---:|---:|---:|
| Normalizado | Nenhuma | 40 | 39 de 39 | 0 | 0 | 0 | 0 |
| Normalizado | CSP e sandbox | 40 | 39 de 39 | 0 | 0 | 0 | 0 |
| Cru (controle) | Nenhuma | 39 | 39 de 39 | 10 | 15 | 17 | 15 |
| Cru | CSP e sandbox | 39 | 39 de 39 | 0 | 0 | 0 | 0 |
| Cru | Só CSP | 39 | 39 de 39 | 0 | 0 | 0 | 0 |
| Cru | Só sandbox | 39 | 39 de 39 | 0 | 0 | 16 | 0 |

O controle mostra que o teste prova alguma coisa: com o livro cru e nenhuma barreira, 15 vetores rodam e 17 pedidos
saem para a rede. Com o livro normalizado e nenhuma barreira, nada. O normalizador é, sozinho, uma barreira inteira;
a CSP e o sandbox continuam ligados no app.

A primeira rodada acusou "39 de 40 seções carregadas". A 40ª é o `evil-frame.xhtml`, que no livro original não está no
spine: é o alvo do vetor `frame-chapter`. O normalizador guarda os documentos fora do spine como seções não lineares,
para que links ainda cheguem a eles, e este, sem o script neutralizado, fica vazio e sem o marcador de capítulo. O
teste agora conta os capítulos e exige que a única seção sem marcador seja essa; os números da tabela são da segunda
rodada, com os binários da Parte 1.

### 5.5 Relayout com as seções cortadas

O normalizador garante o tamanho: no acervo, nenhuma parte passa de 203.235 bytes; no `large.epub`, as seções de 1 e 3
MiB viram partes de 171 e de 188 a 193 KiB. Na Fase 0, 200 KiB por seção levaram de 27 a 65 ms no P95 em todas as
mudanças (ADR 0001, tabela 5.4).

**Na janela real** (2026-11-09, computador livre e sem input do começo ao fim, binários da Parte 1). O benchmark da
Fase 0 (`tools/run-epub-bench.mjs`) no `large.epub` normalizado, que agora tem 68 seções; ele mede a menor, a mediana
e a maior, 20 vezes cada mudança. Tempo até o quadro final na tela, P95 e máximo, em ms:

| Seção | Fonte, paginado | Fonte, contínuo | Largura, paginado | Largura, contínuo | Janela, paginado | Janela, contínuo |
|---|---:|---:|---:|---:|---:|---:|
| 21 KiB | 16 (26) | 27 (27) | 25 (27) | 39 (40) | 31 (40) | 30 (31) |
| 31 KiB (mediana) | 13 (26) | 15 (27) | 27 (27) | 41 (41) | 31 (31) | 30 (31) |
| 193 KiB (a maior) | 40 (40) | 37 (40) | 53 (57) | 70 (72) | 68 (72) | 33 (33) |

Todas as 18 combinações ficam abaixo de 100 ms, e a posição ficou no lugar nas 360 mudanças. A seção de 193 KiB leva o
mesmo que a de 200 KiB da Fase 0 (de 27 a 65 ms no P95), e as seções de 1 e 3 MiB que levavam de 164 a 1.100 ms não
existem mais. O pior caso de todos foi 72 ms.

### 5.6 Arquivos do Calibre

Dois livros do Projeto Gutenberg convertidos pelo Calibre 9.16, escritor independente do meu:

| Arquivo | Lido como | Documentos | Sumário | Posições |
|---|---|---:|---:|---:|
| Moby Dick, MOBI 6 (`--mobi-file-type=old`) | MOBI 6 | 145 | 145 | 20.043 |
| Moby Dick, MOBI conjunto (`both`) | sua metade KF8 | 144 | 145 | 30.407 |
| Moby Dick, AZW3 | KF8 | 144 | 145 | 30.407 |
| Pride and Prejudice, AZW3 | KF8 | 77 | 63 | 25.828 |
| Pride and Prejudice, FB2 | FB2 | 9 | 0 (as seções do Calibre não têm título) | 20.456 |
| Moby Dick, TXT | TXT (UTF-8) | 138 | 137 | 11.654 |

O primeiro MOBI 6 do Calibre achou um defeito: o html5ever lê o `<mbp:pagebreak/>` como um elemento que nunca fecha, e o
livro inteiro sumia dentro da primeira quebra. As tags `mbp:` agora são trocadas antes do parse (D8). O TXT achou outro:
as linhas do sumário viravam capítulos vazios; três ou mais títulos seguidos agora são um sumário.

## 6. Limitações conhecidas

- **O leitor ainda não usa o pacote normalizado.** O app da Fase 0 abre EPUB pelo `zip.js` do foliate-js; ligá-lo ao
  cache do normalizador, aos CFIs do leitor e ao Locator é a Parte 2. Os testes de janela usam o pacote normalizado
  empacotado como EPUB.
- **Hiperlinks para a web ficam** (D5), passivos: o leitor intercepta o clique (seção 8).
- **Partes repetem o contêiner.** Um seletor como `.capitulo > p:first-child` passa a casar com o primeiro parágrafo de
  cada parte; o Calibre tem o mesmo efeito ao dividir.
- **HUFF/CDIC** sem arquivo real de teste (D8). MOBI com DRM é recusado.
- **Imagens de Markdown** só como `data:`: o worker vê um arquivo e nada ao lado dele.
- **TXT** reconhece capítulos só com palavra-chave e número; um livro sem isso é um documento só, cortado por tamanho.
- **FB2 sem `<title>` nas seções** (como o do Calibre) fica sem sumário.
- **Memória.** O livro inteiro e o pacote normalizado ficam em memória no worker (limite de 3 GiB no job) e no host
  (até 1 GiB por livro).
- **Uma tabela ou bloco único acima de 200 KiB** fica numa parte acima do orçamento (contada; nenhuma no acervo).

## 7. Como reproduzir

```powershell
cargo test --workspace
cargo build --release -p corpus-check -p musaeum-library -p musaeum-cli --bins
cargo build --release -p gen-fixtures -p musaeum-flow --examples
.\target\release\gen-fixtures.exe --out fixtures --only flow
.\target\release\corpus-check.exe --corpus D:\Livros --work D:\Temp\fluido --flow-only
node tools\cfi-browser-check.mjs --cache D:\Temp\fluido\cache
.\target\release\musaeum.exe normalize fixtures\large.epub D:\Temp\large-normalizado.epub
node tools\run-epub-bench.mjs --epub D:\Temp\large-normalizado.epub --out layout-normalizado
node tools\test-epub-security.mjs --normalized
```

## 8. Decisões da auditoria (2026-11-09)

O projeto auditou a Parte 1 e a aprovou. As quatro decisões que estavam em aberto ficaram assim:

1. **Hiperlinks para a web ficam, passivos** (D5). Os links `http(s)`, `mailto` e `tel` continuam links, que não
   carregam nada sozinhos; o leitor intercepta o clique e decide o que fazer com ele. Tudo que carrega sem clique
   continua removido.
2. **O CFI canônico conta nos documentos originais do livro** (D6), e o CFI do leitor, nas partes; a conversão entre
   os dois é aritmética.
3. **Detecção conservadora de capítulos em TXT**: só linhas com palavra-chave e número, e três ou mais títulos
   seguidos são um sumário, não capítulos.
4. **`EXTRACTOR_VERSION` sobe para 3** (D11), e os metadados de MOBI, AZW3 e FB2 são lidos de novo na próxima
   reconciliação.

Os dois testes na janela real que faltavam rodaram depois da aprovação, com os mesmos binários (seções 5.4 e 5.5).
