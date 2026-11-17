//! A book for the OCR and the line detector: pages of real text, in Portuguese, whose every line is
//! known exactly, written as a PDF with a text layer, the *native* book. `visual-check scan` draws
//! it with PDFium and turns it into its scanned twin, a PDF of pictures only: whatever the reader
//! selects on one it should select on the other, and the twin's OCR should read what the native
//! book's text layer says.
//!
//! Four pages: prose in one column; two columns over a halftone photograph with its caption, a
//! caption set at 90° beside it, a bar chart whose axis titles run at 90° and whose month labels run
//! at 45°, and a table ruled into cells; blocks of text turned 90°, 270°, 45° and −30°; and a page of
//! prose for the scans that curl and skew. The fonts are two of PDF's standard fonts, Times-Roman and
//! Helvetica, in WinAnsiEncoding with their widths written into the file, so where each line ends is
//! arithmetic here and the same arithmetic in PDFium.

use std::fmt::Write as _;
use std::io;

use crate::{CATALOG_ID, PAGES_ID, PdfWriter, XorShift, deflate};

/// US Letter, in points.
pub const PAGE: [f32; 2] = [612.0, 792.0];

/// What a line of text is on its page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Heading,
    Body,
    Caption,
    /// A tick, a category, a title of a chart axis.
    Label,
    /// The text of a table's cell.
    Cell,
    /// A line of a block turned away from the page's lines.
    Turned,
    Folio,
}

impl LineKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Heading => "heading",
            Self::Body => "body",
            Self::Caption => "caption",
            Self::Label => "label",
            Self::Cell => "cell",
            Self::Turned => "turned",
            Self::Folio => "folio",
        }
    }
}

/// A line of text as written.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub page: u32,
    pub text: String,
    /// Where its baseline starts and ends, in points from the page's bottom-left corner.
    pub start: [f32; 2],
    pub end: [f32; 2],
    /// The size of its font, in points.
    pub size: f32,
    pub kind: LineKind,
    /// Its block: lines of one block are read one after another.
    pub block: u32,
}

impl OcrLine {
    /// The direction it runs in, degrees counterclockwise from the page's x axis, as PDF has it.
    pub fn angle_degrees(&self) -> f32 {
        (self.end[1] - self.start[1])
            .atan2(self.end[0] - self.start[0])
            .to_degrees()
    }
}

/// What is on a page and is not text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphicKind {
    Photograph,
    Chart,
    Table,
}

impl GraphicKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Photograph => "photograph",
            Self::Chart => "chart",
            Self::Table => "table",
        }
    }
}

/// A picture, a chart's plot or a table's rules: where a found line that is no line of text would
/// be a false one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OcrGraphic {
    pub page: u32,
    /// Left, bottom, right, top, in points.
    pub rect: [f32; 4],
    pub kind: GraphicKind,
}

/// The native book and its answers.
#[derive(Clone, Debug)]
pub struct OcrBook {
    pub pdf: Vec<u8>,
    pub lines: Vec<OcrLine>,
    pub graphics: Vec<OcrGraphic>,
}

/// The two fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Font {
    Times,
    Helvetica,
}

const TIMES: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
];

const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 556, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// The letter an accented letter of Latin-1 is drawn on, for its width.
fn base_letter(ch: char) -> char {
    match ch {
        'À'..='Å' => 'A',
        'Ç' => 'C',
        'È'..='Ë' => 'E',
        'Ì'..='Ï' => 'I',
        'Ñ' => 'N',
        'Ò'..='Ö' => 'O',
        'Ù'..='Ü' => 'U',
        'Ý' => 'Y',
        'à'..='å' => 'a',
        'ç' => 'c',
        'è'..='ë' => 'e',
        'ì'..='ï' => 'i',
        'ñ' => 'n',
        'ò'..='ö' => 'o',
        'ù'..='ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

impl Font {
    fn widths(self) -> &'static [u16; 95] {
        match self {
            Self::Times => &TIMES,
            Self::Helvetica => &HELVETICA,
        }
    }

    /// The width of a character of WinAnsiEncoding, in thousandths of the font's size.
    fn width(self, ch: char) -> u16 {
        let base = base_letter(ch);
        let code = base as u32;
        if (32..127).contains(&code) {
            self.widths()[(code - 32) as usize]
        } else {
            500
        }
    }

    fn text_width(self, text: &str, size: f32) -> f32 {
        text.chars()
            .map(|ch| f32::from(self.width(ch)))
            .sum::<f32>()
            * size
            / 1000.0
    }

    fn resource(self) -> &'static str {
        match self {
            Self::Times => "F1",
            Self::Helvetica => "F2",
        }
    }

    fn base_font(self) -> &'static str {
        match self {
            Self::Times => "Times-Roman",
            Self::Helvetica => "Helvetica",
        }
    }

    /// `/Widths` from 32 to 255, the accented letters as wide as their letters.
    fn widths_array(self) -> String {
        let mut out = String::from("[");
        for code in 32u32..=255 {
            let ch = char::from_u32(code).unwrap_or(' ');
            let width = if (127..160).contains(&code) {
                0
            } else {
                self.width(ch)
            };
            let _ = write!(out, "{width} ");
        }
        out.push(']');
        out
    }
}

/// `text` as a PDF string in WinAnsiEncoding: Latin-1's letters are the same codes.
fn pdf_string(text: &str) -> String {
    let mut out = String::from("(");
    for ch in text.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            ' '..='~' => out.push(ch),
            _ => {
                let code = (ch as u32).min(255);
                let _ = write!(out, "\\{code:03o}");
            }
        }
    }
    out.push(')');
    out
}

/// Sentences the pages are filled with, original and plain, with the accents Portuguese has.
const PROSE: [&str; 30] = [
    "A biblioteca da cidade abriu as portas numa manhã de chuva, com três salas de leitura e um catálogo escrito à mão.",
    "Os primeiros leitores eram estudantes, comerciantes e algumas senhoras que vinham buscar romances de capa dura.",
    "Cada livro recebia um número, uma ficha e um lugar na estante, e a ordem das fichas era a memória da casa.",
    "Quando faltava um volume, o bibliotecário consultava o caderno de empréstimos e escrevia uma carta ao leitor.",
    "No inverno a sala grande ficava fria, e os jornais do dia eram lidos de casaco junto às janelas altas.",
    "A coleção cresceu com doações de famílias que mudavam de casa e não tinham onde guardar os livros do avô.",
    "Vieram dicionários, atlas, manuais de agricultura, revistas ilustradas e uma enciclopédia em vinte volumes.",
    "Para encontrar um assunto era preciso conhecer o índice, as remissões e a paciência de quem cuidava do acervo.",
    "Os mapas antigos ficavam numa gaveta larga, e só eram abertos sobre a mesa coberta com um pano verde.",
    "Na década seguinte a câmara municipal comprou um prédio novo, com estantes de ferro e luz elétrica.",
    "A mudança levou dois meses, e nenhum volume se perdeu, embora muitos tenham voltado com as lombadas gastas.",
    "Os encadernadores trabalhavam no porão, costurando cadernos soltos e colando capas de percalina azul.",
    "Uma estudante de história organizou as cartas do fundador, que estavam guardadas em caixas de sapato.",
    "Ela descobriu que a primeira lista de compras incluía gramáticas, tratados de botânica e poesia francesa.",
    "Ler em voz alta era costume nas tardes de sábado, quando os mais velhos traziam os netos para ouvir.",
    "Com o tempo as fichas de papel deram lugar a um sistema que mostrava na tela onde estava cada obra.",
    "Mesmo assim, os leitores antigos preferiam perguntar ao balcão, porque a resposta vinha com uma história.",
    "As obras raras passaram a ser fotografadas página por página, para que o original descansasse no cofre.",
    "Uma página fotografada não tem texto, só a imagem das letras, e por isso não se pode copiar nem pesquisar.",
    "O reconhecimento óptico de caracteres lê essa imagem e devolve as palavras com a posição de cada uma.",
    "Quando a leitura acerta, a página escaneada se comporta como um livro digital, e a busca encontra o trecho.",
    "Quando erra, troca letras parecidas, junta palavras ou inventa sinais onde havia uma mancha no papel.",
    "Por isso cada palavra vem com uma confiança, e o que tem confiança baixa merece um segundo olhar.",
    "Fotografias, gráficos e tabelas pedem cuidado, porque as suas linhas e pontos não são texto.",
    "Uma legenda ao lado da figura, escrita de baixo para cima, também é texto e deve poder ser selecionada.",
    "Os rótulos de um gráfico, inclinados para caber sob as barras, são lidos depois de endireitados.",
    "A tabela tem fios que separam as células, e o texto de cada célula é lido sem os fios.",
    "No fim do ano, o relatório contou quantos livros saíram, quantos voltaram e quantos ainda faltavam.",
    "O número de visitantes dobrou depois que a sala infantil ganhou tapetes, almofadas e uma estante baixa.",
    "Hoje a biblioteca guarda também os livros digitais, e as páginas antigas ganharam texto que se pode copiar.",
];

/// What a page is drawn with, as it is written.
struct Page {
    content: String,
    /// The photograph, if this page draws it.
    photo: bool,
}

/// Lays out lines of text in a column, word by word, from the sentences starting at `from`.
struct Writer<'a> {
    lines: &'a mut Vec<OcrLine>,
    content: String,
    page: u32,
    block: u32,
}

impl Writer<'_> {
    /// One line from `start`, turned by `angle` degrees counterclockwise.
    fn line(
        &mut self,
        text: &str,
        font: Font,
        size: f32,
        start: [f32; 2],
        angle: f32,
        kind: LineKind,
    ) {
        let (sin, cos) = angle.to_radians().sin_cos();
        let width = font.text_width(text, size);
        let _ = writeln!(
            self.content,
            "BT /{} {size} Tf {cos:.5} {sin:.5} {:.5} {cos:.5} {:.3} {:.3} Tm {} Tj ET",
            font.resource(),
            -sin,
            start[0],
            start[1],
            pdf_string(text)
        );
        self.lines.push(OcrLine {
            page: self.page,
            text: text.to_owned(),
            start,
            end: [start[0] + width * cos, start[1] + width * sin],
            size,
            kind,
            block: self.block,
        });
    }

    /// A paragraph from `words`, set ragged right in `width` points from `left`, its first line's
    /// baseline at `top`, `leading` apart, at most `max_lines` lines; returns the words left over
    /// and the next baseline.
    #[allow(clippy::too_many_arguments)]
    fn paragraph<'w>(
        &mut self,
        words: &'w [&'w str],
        font: Font,
        size: f32,
        left: f32,
        width: f32,
        top: f32,
        leading: f32,
        max_lines: usize,
    ) -> (&'w [&'w str], f32) {
        let mut at = 0;
        let mut baseline = top;
        let mut count = 0;
        while at < words.len() && count < max_lines {
            let mut line = String::new();
            let mut next = at;
            while next < words.len() {
                let candidate = if line.is_empty() {
                    words[next].to_owned()
                } else {
                    format!("{line} {}", words[next])
                };
                if font.text_width(&candidate, size) > width && !line.is_empty() {
                    break;
                }
                line = candidate;
                next += 1;
            }
            self.line(&line, font, size, [left, baseline], 0.0, LineKind::Body);
            at = next;
            baseline -= leading;
            count += 1;
        }
        (&words[at..], baseline)
    }
}

/// The words of the sentences from `first`, as many as there are, wrapping around.
fn words_from(first: usize, sentences: usize) -> Vec<&'static str> {
    (0..sentences)
        .flat_map(|index| PROSE[(first + index) % PROSE.len()].split(' '))
        .collect()
}

/// A halftone photograph: soft light and shade with a few dark shapes, printed as dots on a
/// screen turned 45°, as a book's plates are. One byte a pixel.
fn photograph(width: u32, height: u32, seed: u64) -> Vec<u8> {
    let mut rng = XorShift::new(seed);
    let mut pixels = vec![0u8; (width * height) as usize];
    let cell = 5.0f32;
    let (sin, cos) = 45f32.to_radians().sin_cos();
    for y in 0..height {
        for x in 0..width {
            let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
            let mut tone =
                0.55 + 0.25 * (u * 6.0).sin() * (v * 4.0).cos() + 0.12 * (u * 15.0 + v * 9.0).sin();
            if (u - 0.3).powi(2) + (v - 0.55).powi(2) < 0.03
                || ((u - 0.72).abs() < 0.05 && v > 0.25)
            {
                tone = 0.15;
            }
            let tone = tone.clamp(0.02, 0.98);
            // The screen: a dot at every cell's centre, as big as the shade is dark.
            let (rx, ry) = (
                x as f32 * cos - y as f32 * sin,
                x as f32 * sin + y as f32 * cos,
            );
            let (fx, fy) = (
                (rx / cell).fract().abs() - 0.5,
                (ry / cell).fract().abs() - 0.5,
            );
            let distance = (fx * fx + fy * fy).sqrt();
            let radius = ((1.0 - tone) / std::f32::consts::PI).sqrt();
            let ink = distance < radius;
            let grain = rng.below(12) as f32 - 6.0;
            let value = if ink { 35.0 } else { 236.0 } + grain;
            pixels[(y * width + x) as usize] = value.clamp(0.0, 255.0) as u8;
        }
    }
    pixels
}

/// The native book.
pub fn ocr_book() -> io::Result<OcrBook> {
    let mut lines = Vec::new();
    let mut graphics = Vec::new();
    let mut pages = Vec::new();
    let [page_width, page_height] = PAGE;

    // Page 1: prose in one column.
    {
        let mut writer = Writer {
            lines: &mut lines,
            content: String::new(),
            page: 0,
            block: 0,
        };
        writer.line(
            "Memória de uma biblioteca",
            Font::Helvetica,
            16.0,
            [72.0, 720.0],
            0.0,
            LineKind::Heading,
        );
        writer.block += 1;
        let words = words_from(0, 30);
        let mut rest: &[&str] = &words;
        let mut top = 690.0;
        for paragraph in 0..4 {
            let (left, next) = writer.paragraph(
                rest,
                Font::Times,
                11.0,
                72.0,
                468.0,
                top,
                14.5,
                [9, 8, 9, 10][paragraph],
            );
            rest = left;
            top = next - 8.0;
            writer.block += 1;
        }
        writer.line("1", Font::Times, 10.0, [303.0, 50.0], 0.0, LineKind::Folio);
        pages.push(Page {
            content: writer.content,
            photo: false,
        });
    }

    // Page 2: two columns over a photograph, a chart and a table.
    {
        let mut writer = Writer {
            lines: &mut lines,
            content: String::new(),
            page: 1,
            block: 0,
        };
        writer.line(
            "Coleções e figuras",
            Font::Helvetica,
            14.0,
            [72.0, 730.0],
            0.0,
            LineKind::Heading,
        );
        writer.block += 1;
        let words = words_from(12, 16);
        let (rest, _) = writer.paragraph(&words, Font::Times, 10.0, 72.0, 222.0, 705.0, 13.0, 12);
        writer.block += 1;
        writer.paragraph(rest, Font::Times, 10.0, 318.0, 222.0, 705.0, 13.0, 12);
        writer.block += 1;
        // The photograph across the page, its caption below and a caption at 90° beside it.
        let photo = [72.0, 395.0, 500.0, 545.0];
        let _ = writeln!(
            writer.content,
            "q {} 0 0 {} {} {} cm /Im0 Do Q",
            photo[2] - photo[0],
            photo[3] - photo[1],
            photo[0],
            photo[1]
        );
        graphics.push(OcrGraphic {
            page: 1,
            rect: photo,
            kind: GraphicKind::Photograph,
        });
        writer.line(
            "Figura 1. A sala de leitura no inverno de 1932.",
            Font::Helvetica,
            8.5,
            [72.0, 382.0],
            0.0,
            LineKind::Caption,
        );
        writer.block += 1;
        writer.line(
            "Fotografia do acervo municipal",
            Font::Helvetica,
            8.0,
            [514.0, 400.0],
            90.0,
            LineKind::Caption,
        );
        writer.block += 1;

        // The chart: a plot with axes, grid lines and bars, its labels around it.
        let plot = [100.0f32, 120.0, 300.0, 330.0];
        graphics.push(OcrGraphic {
            page: 1,
            rect: plot,
            kind: GraphicKind::Chart,
        });
        let mut chart = String::new();
        let _ = writeln!(chart, "q 0.7 G 0.4 w");
        for step in 1..=4 {
            let y = plot[1] + (plot[3] - plot[1]) * step as f32 / 4.0;
            let _ = writeln!(chart, "{} {y} m {} {y} l S", plot[0], plot[2]);
        }
        let _ = writeln!(
            chart,
            "Q q 0 G 1 w {} {} m {} {} l {} {} l S Q",
            plot[0], plot[3], plot[0], plot[1], plot[2], plot[1]
        );
        let heights = [0.45f32, 0.7, 0.55, 0.9, 0.35, 0.62];
        let months = ["Janeiro", "Fevereiro", "Março", "Abril", "Maio", "Junho"];
        let bar = (plot[2] - plot[0]) / heights.len() as f32;
        for (index, height) in heights.iter().enumerate() {
            let x = plot[0] + bar * index as f32 + bar * 0.2;
            let _ = writeln!(
                chart,
                "q 0.35 g {x} {} {} {} re f Q",
                plot[1],
                bar * 0.6,
                (plot[3] - plot[1]) * height
            );
        }
        // Ticks on the y axis.
        for step in 0..=4 {
            let y = plot[1] + (plot[3] - plot[1]) * step as f32 / 4.0;
            let _ = writeln!(
                chart,
                "q 0 G 0.8 w {} {y} m {} {y} l S Q",
                plot[0] - 4.0,
                plot[0]
            );
        }
        writer.content.push_str(&chart);
        for step in 0..=4 {
            let y = plot[1] + (plot[3] - plot[1]) * step as f32 / 4.0;
            let label = format!("{}", step * 25);
            let width = Font::Helvetica.text_width(&label, 7.0);
            writer.line(
                &label,
                Font::Helvetica,
                7.0,
                [plot[0] - 7.0 - width, y - 2.5],
                0.0,
                LineKind::Label,
            );
            writer.block += 1;
        }
        for (index, month) in months.iter().enumerate() {
            let x = plot[0] + bar * (index as f32 + 0.5);
            // Turned 45°, ending under its bar.
            let width = Font::Helvetica.text_width(month, 7.5);
            let (sin, cos) = 45f32.to_radians().sin_cos();
            let start = [x - width * cos, plot[1] - 8.0 - width * sin];
            writer.line(month, Font::Helvetica, 7.5, start, 45.0, LineKind::Label);
            writer.block += 1;
        }
        writer.line(
            "Empréstimos por mês",
            Font::Helvetica,
            8.0,
            [72.0, 170.0],
            90.0,
            LineKind::Label,
        );
        writer.block += 1;

        // The table: three columns, four rows, ruled.
        let table = [340.0f32, 150.0, 540.0, 310.0];
        graphics.push(OcrGraphic {
            page: 1,
            rect: table,
            kind: GraphicKind::Table,
        });
        let rows = [
            ["Sala", "Volumes", "Leitores"],
            ["Geral", "12.480", "3.150"],
            ["Infantil", "2.310", "1.870"],
            ["Raros", "640", "95"],
        ];
        let row_height = (table[3] - table[1]) / rows.len() as f32;
        let columns = [table[0], table[0] + 80.0, table[0] + 140.0, table[2]];
        let mut rules = String::from("q 0 G 0.8 w\n");
        for row in 0..=rows.len() {
            let y = table[1] + row_height * row as f32;
            let _ = writeln!(rules, "{} {y} m {} {y} l S", table[0], table[2]);
        }
        for x in columns {
            let _ = writeln!(rules, "{x} {} m {x} {} l S", table[1], table[3]);
        }
        rules.push_str("Q\n");
        writer.content.push_str(&rules);
        // Each cell is written on its own, and a row is one line across its cells, as a text layer
        // reads it and the reader selects it.
        for (row, cells) in rows.iter().enumerate() {
            let baseline = table[3] - row_height * (row as f32 + 1.0) + row_height * 0.35;
            let first = writer.lines.len();
            for (column, cell) in cells.iter().enumerate() {
                writer.line(
                    cell,
                    Font::Helvetica,
                    9.0,
                    [columns[column] + 6.0, baseline],
                    0.0,
                    LineKind::Cell,
                );
            }
            let end = writer.lines[writer.lines.len() - 1].end;
            let start = writer.lines[first].start;
            writer.lines.truncate(first);
            writer.lines.push(OcrLine {
                page: writer.page,
                text: cells.join(" "),
                start,
                end,
                size: 9.0,
                kind: LineKind::Cell,
                block: writer.block,
            });
            writer.block += 1;
        }
        writer.line("2", Font::Times, 10.0, [303.0, 50.0], 0.0, LineKind::Folio);
        pages.push(Page {
            content: writer.content,
            photo: true,
        });
    }

    // Page 3: blocks of text turned away from the page.
    {
        let mut writer = Writer {
            lines: &mut lines,
            content: String::new(),
            page: 2,
            block: 0,
        };
        writer.line(
            "Textos em outros ângulos",
            Font::Helvetica,
            14.0,
            [150.0, 730.0],
            0.0,
            LineKind::Heading,
        );
        writer.block += 1;
        let turned = |writer: &mut Writer<'_>,
                      first: usize,
                      origin: [f32; 2],
                      angle: f32,
                      count: usize,
                      length: f32| {
            let words = words_from(first, 4);
            let (sin, cos) = angle.to_radians().sin_cos();
            // Lines stack to the right of the reading direction: down the page for a level line.
            let (nx, ny) = (sin, -cos);
            let mut at = 0;
            for line in 0..count {
                let mut text = String::new();
                while at < words.len() {
                    let candidate = if text.is_empty() {
                        words[at].to_owned()
                    } else {
                        format!("{text} {}", words[at])
                    };
                    if Font::Times.text_width(&candidate, 10.0) > length && !text.is_empty() {
                        break;
                    }
                    text = candidate;
                    at += 1;
                }
                let offset = 13.0 * line as f32;
                writer.line(
                    &text,
                    Font::Times,
                    10.0,
                    [origin[0] + nx * offset, origin[1] + ny * offset],
                    angle,
                    LineKind::Turned,
                );
            }
            writer.block += 1;
        };
        // Up the left margin, reading from the bottom.
        turned(&mut writer, 3, [80.0, 120.0], 90.0, 5, 520.0);
        // Down the right margin, reading from the top.
        turned(&mut writer, 7, [532.0, 680.0], -90.0, 5, 520.0);
        // At 45°, in the middle.
        turned(&mut writer, 11, [190.0, 330.0], 45.0, 4, 250.0);
        // At −30°.
        turned(&mut writer, 15, [200.0, 640.0], -30.0, 4, 230.0);
        // Level lines under them, for the order of reading.
        let words = words_from(19, 2);
        writer.paragraph(&words, Font::Times, 10.0, 170.0, 330.0, 150.0, 13.0, 4);
        writer.block += 1;
        writer.line("3", Font::Times, 10.0, [303.0, 50.0], 0.0, LineKind::Folio);
        pages.push(Page {
            content: writer.content,
            photo: false,
        });
    }

    // Page 4: prose for the scans that curl and skew.
    {
        let mut writer = Writer {
            lines: &mut lines,
            content: String::new(),
            page: 3,
            block: 0,
        };
        writer.line(
            "O acervo fotografado",
            Font::Helvetica,
            16.0,
            [72.0, 720.0],
            0.0,
            LineKind::Heading,
        );
        writer.block += 1;
        let words = words_from(17, 30);
        let mut rest: &[&str] = &words;
        let mut top = 690.0;
        for count in [10, 9, 10, 9] {
            let (left, next) =
                writer.paragraph(rest, Font::Times, 11.0, 72.0, 468.0, top, 14.5, count);
            rest = left;
            top = next - 8.0;
            writer.block += 1;
        }
        writer.line("4", Font::Times, 10.0, [303.0, 50.0], 0.0, LineKind::Folio);
        pages.push(Page {
            content: writer.content,
            photo: false,
        });
    }

    // The file.
    let (photo_width, photo_height) = (428u32, 150u32);
    let photo = deflate(&photograph(photo_width, photo_height, 41))?;
    let mut pdf = PdfWriter {
        out: Vec::new(),
        position: 0,
        offsets: vec![0],
    };
    pdf.bytes(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n")?;
    let fonts_id = 3usize;
    let photo_id = 5usize;
    let first_page = 6usize;
    pdf.object(
        CATALOG_ID,
        &format!("<< /Type /Catalog /Pages {PAGES_ID} 0 R >>"),
    )?;
    let kids: String = (0..pages.len())
        .map(|index| format!("{} 0 R ", first_page + index * 2))
        .collect();
    pdf.object(
        PAGES_ID,
        &format!("<< /Type /Pages /Count {} /Kids [ {kids}] >>", pages.len()),
    )?;
    for (offset, font) in [Font::Times, Font::Helvetica].into_iter().enumerate() {
        pdf.object(
            fonts_id + offset,
            &format!(
                "<< /Type /Font /Subtype /Type1 /BaseFont /{} /Encoding /WinAnsiEncoding \
                 /FirstChar 32 /LastChar 255 /Widths {} >>",
                font.base_font(),
                font.widths_array()
            ),
        )?;
    }
    pdf.stream(
        photo_id,
        &format!(
            "/Type /XObject /Subtype /Image /Width {photo_width} /Height {photo_height} \
             /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode"
        ),
        &photo,
    )?;
    for (index, page) in pages.iter().enumerate() {
        let id = first_page + index * 2;
        let xobject = if page.photo {
            format!(" /XObject << /Im0 {photo_id} 0 R >>")
        } else {
            String::new()
        };
        pdf.object(
            id,
            &format!(
                "<< /Type /Page /Parent {PAGES_ID} 0 R /MediaBox [0 0 {page_width} {page_height}] \
                 /Resources << /Font << /F1 {} 0 R /F2 {} 0 R >>{xobject} >> /Contents {} 0 R >>",
                fonts_id,
                fonts_id + 1,
                id + 1
            ),
        )?;
        pdf.stream(
            id + 1,
            "/Filter /FlateDecode",
            &deflate(page.content.as_bytes())?,
        )?;
    }
    let pdf = pdf.finish()?;
    Ok(OcrBook {
        pdf,
        lines,
        graphics,
    })
}

/// The answers as JSON, for tools outside Rust: every line and every graphic, in points.
pub fn truth_json(book: &OcrBook) -> String {
    let escape = |text: &str| {
        let mut out = String::new();
        for ch in text.chars() {
            match ch {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                _ => out.push(ch),
            }
        }
        out
    };
    let mut out = format!("{{\"page\":[{},{}],\"lines\":[", PAGE[0], PAGE[1]);
    for (index, line) in book.lines.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"page\":{},\"text\":\"{}\",\"start\":[{:.3},{:.3}],\"end\":[{:.3},{:.3}],\"size\":{},\"kind\":\"{}\",\"block\":{}}}",
            line.page,
            escape(&line.text),
            line.start[0],
            line.start[1],
            line.end[0],
            line.end[1],
            line.size,
            line.kind.name(),
            line.block
        );
    }
    out.push_str("],\"graphics\":[");
    for (index, graphic) in book.graphics.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"page\":{},\"rect\":[{},{},{},{}],\"kind\":\"{}\"}}",
            graphic.page,
            graphic.rect[0],
            graphic.rect[1],
            graphic.rect[2],
            graphic.rect[3],
            graphic.kind.name()
        );
    }
    out.push_str("]}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_book_is_the_same_every_time_and_knows_its_lines() {
        let a = ocr_book().unwrap();
        let b = ocr_book().unwrap();
        assert_eq!(a.pdf, b.pdf);
        assert!(a.pdf.starts_with(b"%PDF-1.7"));
        let body = a
            .lines
            .iter()
            .filter(|line| line.kind == LineKind::Body)
            .count();
        assert!(body > 70, "{body}");
        // Lines of the prose stay inside their column.
        for line in a
            .lines
            .iter()
            .filter(|line| line.kind == LineKind::Body && line.page == 0)
        {
            assert!(line.end[0] <= 540.5, "{line:?}");
        }
        let turned: Vec<f32> = a
            .lines
            .iter()
            .filter(|line| line.kind == LineKind::Turned)
            .map(OcrLine::angle_degrees)
            .collect();
        for angle in [90.0, -90.0, 45.0, -30.0] {
            assert!(
                turned.iter().any(|found| (found - angle).abs() < 0.01),
                "{angle}"
            );
        }
        assert_eq!(a.graphics.len(), 3);
        assert!(truth_json(&a).contains("\"kind\":\"photograph\""));
    }

    #[test]
    fn portuguese_letters_are_written_in_win_ansi() {
        assert_eq!(pdf_string("ação (1)"), "(a\\347\\343o \\(1\\))");
        assert_eq!(Font::Times.width('ã'), Font::Times.width('a'));
    }
}
