//! A made-up catalog that looks enough like a real one where that matters for search.
//!
//! Real titles use some words all the time and most words almost never, and that skew is what makes
//! search slow, because a common word has a long list of works to go through. So words are drawn
//! with that skew, from several languages with their accents and from another script. Everything
//! comes from the entry's number, so the same arguments always give the same library.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use gen_fixtures::books::{EpubMeta, epub_bytes, pdf_bytes};

/// Small deterministic generator, so that a run can be reproduced byte for byte.
pub struct Random(u64);

impl Random {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }

    /// Low numbers much more often than high ones, the way words are used.
    fn skewed(&mut self, limit: usize) -> usize {
        let uniform = (self.next() % 1_000_000) as f64 / 1_000_000.0;
        ((uniform.powi(3) * limit as f64) as usize).min(limit - 1)
    }
}

const WORDS: &[&str] = &[
    "the",
    "de",
    "a",
    "o",
    "la",
    "le",
    "of",
    "and",
    "e",
    "da",
    "do",
    "el",
    "der",
    "die",
    "das",
    "in",
    "história",
    "introdução",
    "manual",
    "guia",
    "curso",
    "teoria",
    "prática",
    "fundamentos",
    "análise",
    "introduction",
    "history",
    "guide",
    "theory",
    "practice",
    "principles",
    "handbook",
    "analysis",
    "historia",
    "teoría",
    "práctica",
    "introducción",
    "principios",
    "métodos",
    "técnicas",
    "histoire",
    "théorie",
    "pratique",
    "méthodes",
    "critique",
    "éléments",
    "économie",
    "société",
    "geschichte",
    "einführung",
    "grundlagen",
    "theorie",
    "praxis",
    "wörterbuch",
    "übersetzung",
    "storia",
    "manuale",
    "filosofia",
    "psicologia",
    "sociologia",
    "matemática",
    "física",
    "química",
    "biologia",
    "geografia",
    "literatura",
    "linguagem",
    "música",
    "pintura",
    "arquitetura",
    "engenharia",
    "medicina",
    "direito",
    "economia",
    "política",
    "educação",
    "comunicação",
    "programming",
    "software",
    "algorithms",
    "systems",
    "networks",
    "databases",
    "security",
    "compilers",
    "operating",
    "distributed",
    "machine",
    "learning",
    "statistics",
    "calculus",
    "algebra",
    "geometry",
    "topology",
    "logic",
    "probability",
    "mechanics",
    "thermodynamics",
    "memórias",
    "póstumas",
    "casmurro",
    "sertões",
    "vidas",
    "secas",
    "capitães",
    "areia",
    "crime",
    "castigo",
    "guerra",
    "paz",
    "irmãos",
    "karamazov",
    "idiota",
    "ana",
    "karenina",
    "quixote",
    "mancha",
    "soledad",
    "años",
    "cien",
    "amor",
    "cólera",
    "tiempos",
    "mundo",
    "misérables",
    "étranger",
    "peste",
    "recherche",
    "temps",
    "perdu",
    "madame",
    "bovary",
    "verwandlung",
    "prozess",
    "zauberberg",
    "faust",
    "buddenbrooks",
    "steppenwolf",
    "война",
    "мир",
    "преступление",
    "наказание",
    "идиот",
    "братья",
    "карамазовы",
    "анна",
    "жизнь",
    "судьба",
    "история",
    "теория",
    "введение",
    "основы",
    "руководство",
    "novo",
    "nova",
    "grande",
    "pequeno",
    "primeiro",
    "segunda",
    "último",
    "breve",
    "completo",
    "new",
    "modern",
    "complete",
    "advanced",
    "essential",
    "practical",
    "applied",
    "general",
    "nuevo",
    "gran",
    "primera",
    "moderna",
    "général",
    "nouvelle",
    "neue",
    "große",
    "kleine",
    "volume",
    "edição",
    "tomo",
    "parte",
    "livro",
    "book",
    "edition",
    "band",
    "libro",
    "livre",
];

const FIRST_NAMES: &[&str] = &[
    "José",
    "João",
    "Maria",
    "Ana",
    "Antônio",
    "Carlos",
    "Paulo",
    "Pedro",
    "Lucas",
    "Luís",
    "Marcos",
    "Fernanda",
    "Beatriz",
    "Clarice",
    "Cecília",
    "Graciliano",
    "Machado",
    "Jorge",
    "Rachel",
    "Lygia",
    "Fiódor",
    "Lev",
    "Anton",
    "Ivan",
    "Nikolai",
    "Aleksandr",
    "Mikhail",
    "Sergei",
    "Vladimir",
    "Miguel",
    "Gabriel",
    "Julio",
    "Mario",
    "Octavio",
    "Isabel",
    "Laura",
    "Pablo",
    "Jorge Luis",
    "Honoré",
    "Émile",
    "Gustave",
    "Marcel",
    "Albert",
    "Simone",
    "Marguerite",
    "Françoise",
    "René",
    "Johann",
    "Friedrich",
    "Thomas",
    "Hermann",
    "Franz",
    "Günter",
    "Heinrich",
    "Jürgen",
    "Immanuel",
    "William",
    "Charles",
    "Jane",
    "Virginia",
    "George",
    "Ernest",
    "James",
    "Mary",
    "Emily",
    "John",
    "Donald",
    "Andrew",
    "Brian",
    "Dennis",
    "Linus",
    "Ada",
    "Grace",
    "Alan",
    "Edsger",
    "Barbara",
];

const SURNAMES: &[&str] = &[
    "Silva",
    "Santos",
    "Oliveira",
    "Souza",
    "Pereira",
    "Costa",
    "Rodrigues",
    "Almeida",
    "Nascimento",
    "Lima",
    "Araújo",
    "Ribeiro",
    "Carvalho",
    "Gomes",
    "Martins",
    "Rocha",
    "Assis",
    "Lispector",
    "Ramos",
    "Amado",
    "Queiroz",
    "Telles",
    "Meireles",
    "Andrade",
    "Bandeira",
    "Drummond",
    "Dostoiévski",
    "Tolstói",
    "Tchékhov",
    "Turguêniev",
    "Gógol",
    "Púchkin",
    "Bulgákov",
    "Nabókov",
    "Cervantes",
    "García Márquez",
    "Cortázar",
    "Vargas Llosa",
    "Paz",
    "Allende",
    "Neruda",
    "Borges",
    "Balzac",
    "Zola",
    "Flaubert",
    "Proust",
    "Camus",
    "Beauvoir",
    "Duras",
    "Sagan",
    "Descartes",
    "Goethe",
    "Nietzsche",
    "Mann",
    "Hesse",
    "Kafka",
    "Grass",
    "Böll",
    "Habermas",
    "Kant",
    "Müller",
    "Shakespeare",
    "Dickens",
    "Austen",
    "Woolf",
    "Orwell",
    "Hemingway",
    "Joyce",
    "Shelley",
    "Brontë",
    "Knuth",
    "Tanenbaum",
    "Kernighan",
    "Ritchie",
    "Torvalds",
    "Lovelace",
    "Hopper",
    "Turing",
    "Dijkstra",
    "Liskov",
    "Stroustrup",
    "Sedgewick",
    "Cormen",
    "Rivest",
    "Stein",
    "Leiserson",
];

/// The fragments with the longest lists in the indexes, to type as a worst case.
pub const COMMON_FRAGMENTS: &[&str] = &[
    "the", "and", "ist", "sto", "tor", "ria", "ção", "cao", "ica", "ana", "ara", "der", "ein",
    "ent", "ter", "oso", "man", "ual", "gui", "sta", "ron", "lin", "san", "ira", "ova", "ест",
    "ист", "ани",
];

/// Pairs of the most common words of the vocabulary, each typed in full. Each word alone has a long
/// list, and together they match few works, so nothing ends early: the worst case for an AND.
pub fn common_word_pairs(count: usize) -> Vec<String> {
    let common = &WORDS[..16];
    let mut pairs = Vec::with_capacity(count);
    'outer: for first in common {
        for second in common {
            if first == second {
                continue;
            }
            pairs.push(format!("{first} {second}"));
            if pairs.len() == count {
                break 'outer;
            }
        }
    }
    pairs
}

/// Text with its accents taken off, to type the way people do.
pub fn without_accents(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            'á' | 'à' | 'â' | 'ã' | 'ä' => 'a',
            'é' | 'ê' | 'è' => 'e',
            'í' | 'î' => 'i',
            'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'ú' | 'ü' => 'u',
            'ç' => 'c',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// One book.
pub struct Entry {
    pub title: String,
    pub creators: Vec<String>,
    pub is_epub: bool,
    number: usize,
}

/// Every book there is.
pub struct Catalog {
    works: usize,
}

impl Catalog {
    pub fn new(works: usize) -> Self {
        Self { works }
    }

    pub fn len(&self) -> usize {
        self.works
    }

    pub fn entry(&self, number: usize) -> Entry {
        let mut random = Random::new(number as u64 + 1);
        let count = 2 + random.below(5);
        let words: Vec<&str> = (0..count)
            .map(|_| WORDS[random.skewed(WORDS.len())])
            .collect();
        let mut title = words.join(" ");
        if let Some(first) = title.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        let authors = 1 + usize::from(random.below(5) == 0);
        let creators = (0..authors)
            .map(|_| {
                format!(
                    "{} {}",
                    FIRST_NAMES[random.skewed(FIRST_NAMES.len())],
                    SURNAMES[random.below(SURNAMES.len())]
                )
            })
            .collect();
        Entry {
            title,
            creators,
            is_epub: !number.is_multiple_of(5),
            number,
        }
    }

    /// `<one of 40 shelves>/<one of 50 authors' folders>/<book>`, which makes about 2000 folders.
    fn relative_path(&self, entry: &Entry) -> PathBuf {
        let number = entry.number;
        let extension = if entry.is_epub { "epub" } else { "pdf" };
        PathBuf::from(format!("shelf{:02}", number % 40))
            .join(format!("author{:02}", (number / 40) % 50))
            .join(format!("book{number:06}.{extension}"))
    }

    /// Every book as a real file. Most are a few KiB and one in a hundred is larger, so hashing has
    /// something to do besides opening files. Returns how many bytes that came to.
    pub fn write_all(&self, directory: &Path) -> io::Result<u64> {
        let mut total = 0;
        for number in 0..self.works {
            let entry = self.entry(number);
            let path = directory.join(self.relative_path(&entry));
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let padding = if number % 100 == 0 {
                512 * 1024
            } else {
                1024 + number % 4096
            };
            let creators: Vec<&str> = entry.creators.iter().map(String::as_str).collect();
            let identifier = format!("urn:musaeum:bench:{number}");
            let bytes = if entry.is_epub {
                epub_bytes(
                    &EpubMeta {
                        title: &entry.title,
                        creators: &creators,
                        identifier: Some(&identifier),
                        language: Some("pt-BR"),
                        date: Some("2001-01-01"),
                    },
                    padding,
                    number as u64 + 1,
                )
            } else {
                let id = format!("{number:032x}");
                pdf_bytes(Some(&id), padding, number as u64 + 1)
            };
            fs::write(&path, &bytes)?;
            total += bytes.len() as u64;
        }
        Ok(total)
    }

    /// A folder with a lot of the library in it.
    pub fn largest_shelf(&self, directory: &Path) -> PathBuf {
        directory.join("shelf00")
    }
}

/// How many files there are under a folder.
pub fn count_files(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                count_files(&entry.path())
            } else {
                1
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_number_is_always_the_same_book() {
        let catalog = Catalog::new(100);
        assert_eq!(catalog.entry(7).title, catalog.entry(7).title);
        assert_ne!(catalog.entry(7).title, catalog.entry(8).title);
    }

    #[test]
    fn common_words_are_common() {
        let catalog = Catalog::new(10_000);
        let with_the = (0..10_000)
            .filter(|&number| catalog.entry(number).title.to_lowercase().contains("the "))
            .count();
        assert!(with_the > 1000, "{with_the}");
    }

    #[test]
    fn accents_come_off() {
        assert_eq!(without_accents("Introdução à Ótica"), "Introducao a Ótica");
    }

    #[test]
    fn a_first_letter_that_is_not_ascii_is_left_as_it_is() {
        let mut title = String::from("é");
        if let Some(first) = title.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        assert_eq!(title, "é");
    }
}
