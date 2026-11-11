//! Finding a quote again in text that may have changed.
//!
//! Text is compared folded: every run of white space is one space, letters are lower case, and soft
//! hyphens are gone. That alone absorbs what changes most between two revisions of a book, or two
//! versions of the normalizer: line breaks, indentation, spacing, a capital letter. What the quote
//! says before and after (prefix and suffix) picks among several matches, and the place where the
//! annotation was (`hint`) picks among matches the context cannot tell apart.
//!
//! When the quote is not there as it was, a light edit is looked for: the quote's beginning and
//! its end close together, with what is between them near enough to the quote. That match is only
//! ever approximate.

/// How a quote was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// The text is the quote, and nothing else in the text could be it instead.
    Exact,
    /// The text is the quote but its place is a guess among several, or the text is near the quote.
    Approximate,
}

/// What an annotation quotes: the text and what surrounds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quote {
    pub exact: String,
    pub prefix: String,
    pub suffix: String,
}

/// Characters of context kept on each side of a quote.
pub const CONTEXT_CHARS: usize = 32;
/// The least similarity, from 0 to 1, between a quote and the text an edit left in its place.
pub const MIN_SIMILARITY: f64 = 0.7;
/// A quote shorter than this is only ever found as it is: a few characters are found everywhere.
const MIN_FUZZY_CHARS: usize = 8;
/// Matches looked at for one search, nearest the hint first.
const MAX_CANDIDATES: usize = 4096;

fn fold_char(c: char) -> Option<char> {
    if c == '\u{AD}' {
        return None;
    }
    if c.is_whitespace() {
        return Some(' ');
    }
    // Only a lower case of one character keeps the map from folded to original one to one.
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) => Some(one),
        _ => Some(c),
    }
}

/// Folds a short text the way [`Folded`] folds a long one, trimming it.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().filter_map(fold_char) {
        if c == ' ' && (out.is_empty() || out.ends_with(' ')) {
            continue;
        }
        out.push(c);
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// White space made single spaces and trimmed, the case kept: how a quote is shown and kept.
pub fn collapse(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().filter(|&c| c != '\u{AD}') {
        if c.is_whitespace() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The last `count` characters of `text`.
pub fn tail_chars(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().rev().take(count).collect();
    chars.into_iter().rev().collect()
}

/// The first `count` characters of `text`.
pub fn head_chars(text: &str, count: usize) -> String {
    text.chars().take(count).collect()
}

/// A long text, folded, with where each of its characters came from.
pub struct Folded {
    pub text: String,
    /// For each folded character, its byte in `text` and the byte of the original it came from.
    at: Vec<(u32, u32)>,
    original_len: usize,
}

impl Folded {
    pub fn new(original: &str) -> Self {
        let mut text = String::with_capacity(original.len());
        let mut at = Vec::with_capacity(original.len());
        for (byte, c) in original.char_indices() {
            let Some(folded) = fold_char(c) else {
                continue;
            };
            if folded == ' ' && text.ends_with(' ') {
                continue;
            }
            at.push((text.len() as u32, byte as u32));
            text.push(folded);
        }
        Self {
            text,
            at,
            original_len: original.len(),
        }
    }

    /// The folded character at a byte of the folded text.
    fn index_of_byte(&self, byte: usize) -> usize {
        self.at
            .partition_point(|&(folded, _)| (folded as usize) < byte)
    }

    /// The original byte range of folded characters `start..end`.
    fn original(&self, start: usize, end: usize) -> (usize, usize) {
        let from = self
            .at
            .get(start)
            .map_or(self.original_len, |&(_, b)| b as usize);
        let to = self
            .at
            .get(end)
            .map_or(self.original_len, |&(_, b)| b as usize);
        (from, to.max(from))
    }

    /// The folded character an original byte falls on or after.
    pub fn from_original(&self, byte: usize) -> usize {
        self.at
            .partition_point(|&(_, original)| (original as usize) < byte)
    }

    fn chars(&self) -> usize {
        self.at.len()
    }

    /// Every place `needle` starts, as folded character indices, overlapping ones included.
    fn occurrences(&self, needle: &str) -> Vec<usize> {
        let mut found = Vec::new();
        if needle.is_empty() {
            return found;
        }
        let mut from = 0;
        while let Some(at) = self.text[from..].find(needle) {
            let byte = from + at;
            found.push(self.index_of_byte(byte));
            if found.len() >= MAX_CANDIDATES * 4 {
                break;
            }
            from = byte + self.text[byte..].chars().next().map_or(1, char::len_utf8);
        }
        found
    }

    fn slice(&self, start: usize, end: usize) -> &str {
        let from = self
            .at
            .get(start)
            .map_or(self.text.len(), |&(b, _)| b as usize);
        let to = self
            .at
            .get(end)
            .map_or(self.text.len(), |&(b, _)| b as usize);
        &self.text[from..to.max(from)]
    }

    /// How many characters of `prefix` end right before `start`, and of `suffix` start at `end`.
    fn context(&self, start: usize, end: usize, prefix: &str, suffix: &str) -> usize {
        let before = self.slice(0, start);
        let after = self.slice(end, self.chars());
        let behind = before
            .chars()
            .rev()
            .zip(prefix.chars().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let ahead = after
            .chars()
            .zip(suffix.chars())
            .take_while(|(a, b)| a == b)
            .count();
        behind + ahead
    }

    /// Finds `quote`, near the original byte `hint` if there is one, and returns the original byte
    /// range it covers now.
    pub fn find(&self, quote: &Quote, hint: Option<usize>) -> Option<(usize, usize, Quality)> {
        let exact = fold(&quote.exact);
        if exact.is_empty() {
            return None;
        }
        // The context ends at the quote: a space between them is part of neither.
        let prefix = fold(&quote.prefix);
        let suffix = fold(&quote.suffix);
        let hint = hint.map(|byte| self.from_original(byte));
        let distance = |start: usize| hint.map_or(0, |h| start.abs_diff(h));
        let length = exact.chars().count();

        let starts = self.occurrences(&exact);
        if !starts.is_empty() {
            let full = prefix.chars().count() + suffix.chars().count();
            let scored: Vec<(usize, usize)> = starts
                .iter()
                .take(MAX_CANDIDATES)
                .map(|&start| {
                    (
                        self.context_spaced(start, start + length, &prefix, &suffix),
                        start,
                    )
                })
                .collect();
            let top = scored.iter().map(|&(score, _)| score).max()?;
            let start = scored
                .iter()
                .filter(|&&(score, _)| score == top)
                .map(|&(_, start)| start)
                .min_by_key(|&start| distance(start))?;
            let tied = scored.iter().filter(|&&(score, _)| score == top).count();
            let end = start + length;
            // Exact when nothing else could be it: the only match, or the only one in full context.
            let quality = if starts.len() == 1 || (top == full && tied == 1) {
                Quality::Exact
            } else {
                Quality::Approximate
            };
            let (from, to) = self.original(start, end);
            return Some((from, to, quality));
        }
        self.find_edited(&exact, length, &prefix, &suffix, &distance)
    }

    /// How many characters of the prefix end before `start` and of the suffix start after `end`,
    /// each counted on its own, with the one space allowed between the quote and each side.
    fn context_sides(
        &self,
        start: usize,
        end: usize,
        prefix: &str,
        suffix: &str,
    ) -> (usize, usize) {
        let before = if start > 0 && self.slice(start - 1, start) == " " {
            start - 1
        } else {
            start
        };
        let after = if self.slice(end, end + 1) == " " {
            end + 1
        } else {
            end
        };
        let behind = self
            .slice(0, before)
            .chars()
            .rev()
            .zip(prefix.chars().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let ahead = self
            .slice(after, self.chars())
            .chars()
            .zip(suffix.chars())
            .take_while(|(a, b)| a == b)
            .count();
        (behind, ahead)
    }

    /// The context score with the one space allowed between the quote and each side of it.
    fn context_spaced(&self, start: usize, end: usize, prefix: &str, suffix: &str) -> usize {
        let before = if start > 0 && self.slice(start - 1, start) == " " {
            start - 1
        } else {
            start
        };
        let after = if self.slice(end, end + 1) == " " {
            end + 1
        } else {
            end
        };
        self.context(before, after, prefix, suffix)
    }

    /// A light edit: the quote's first and last characters close together, around text near enough
    /// to the quote.
    fn find_edited(
        &self,
        exact: &str,
        length: usize,
        prefix: &str,
        suffix: &str,
        distance: &dyn Fn(usize) -> usize,
    ) -> Option<(usize, usize, Quality)> {
        if length < MIN_FUZZY_CHARS {
            return None;
        }
        let piece = (length / 3).clamp(4, CONTEXT_CHARS);
        let head = head_chars(exact, piece);
        let tail = tail_chars(exact, piece);
        let mut heads = self.occurrences(&head);
        let mut tails = self.occurrences(&tail);
        heads.sort_by_key(|&at| distance(at));
        tails.sort_by_key(|&at| distance(at));
        heads.truncate(MAX_CANDIDATES);
        tails.truncate(MAX_CANDIDATES);
        let shortest = length / 2;
        let longest = length + length / 2 + piece;
        // A candidate held by both ends of the quote, or by one end and what surrounds that end:
        // text that only starts like the quote, with nothing around it the same, is something else
        // that happens to start alike, as the paragraphs of a chapter often do.
        let mut candidates: Vec<(usize, usize, bool)> = Vec::new();
        for &start in &heads {
            let mut paired = false;
            for &at in &tails {
                let end = at + piece;
                if end > start && (shortest..=longest).contains(&(end - start)) {
                    candidates.push((start, end, true));
                    paired = true;
                }
            }
            if !paired {
                candidates.push((start, (start + length).min(self.chars()), false));
            }
        }
        for &at in &tails {
            let end = at + piece;
            if end >= length {
                candidates.push((end - length, end, false));
            }
        }
        let needed = |context: &str| context.chars().count().min(CONTEXT_CHARS) / 2;
        let mut best: Option<(f64, usize, usize, usize)> = None;
        for (start, end, paired) in candidates.into_iter().take(MAX_CANDIDATES) {
            if !paired {
                let (behind, ahead) = self.context_sides(start, end, prefix, suffix);
                let held = (behind >= needed(prefix) && !prefix.is_empty())
                    || (ahead >= needed(suffix) && !suffix.is_empty());
                if !held {
                    continue;
                }
            }
            let found = self.slice(start, end);
            let similarity = similarity(exact, found);
            if similarity < MIN_SIMILARITY {
                continue;
            }
            let score = self.context_spaced(start, end, prefix, suffix);
            let better = match best {
                None => true,
                Some((s, c, b, _)) => {
                    similarity > s + 1e-9
                        || ((similarity - s).abs() <= 1e-9
                            && (score > c || (score == c && distance(start) < distance(b))))
                }
            };
            if better {
                best = Some((similarity, score, start, end));
            }
        }
        let (_, _, start, end) = best?;
        let (from, to) = self.original(start, end);
        Some((from, to, Quality::Approximate))
    }
}

/// How alike two texts are, from 0 to 1: one less the edit distance over the longer length. Texts
/// too long to compare character by character are compared by length and ends.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    if a.len().saturating_mul(b.len()) > 4_000_000 {
        let lengths = 1.0 - a.len().abs_diff(b.len()) as f64 / longest as f64;
        return lengths.min(if a.first() == b.first() && a.last() == b.last() {
            1.0
        } else {
            0.0
        });
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    1.0 - previous[b.len()] as f64 / longest as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(exact: &str, prefix: &str, suffix: &str) -> Quote {
        Quote {
            exact: exact.to_owned(),
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
        }
    }

    const TEXT: &str = "Capitu tinha olhos de ressaca.\n\n   Olhos de cigana oblíqua e dissimulada. \
        Olhos de ressaca, sim, olhos de ressaca.";

    #[test]
    fn folding_absorbs_spacing_case_and_soft_hyphens() {
        assert_eq!(fold("  Olhos\n de\tRES\u{AD}SACA  "), "olhos de ressaca");
        assert_eq!(
            collapse("  Olhos\n de\tRES\u{AD}SACA  "),
            "Olhos de RESSACA"
        );
        let folded = Folded::new("A  b\n\nC");
        assert_eq!(folded.text, "a b c");
    }

    #[test]
    fn an_exact_quote_is_found_with_its_original_bytes() {
        let folded = Folded::new(TEXT);
        let (from, to, quality) = folded
            .find(
                &quote("cigana oblíqua", "Olhos de ", " e dissimulada"),
                None,
            )
            .unwrap();
        assert_eq!(&TEXT[from..to], "cigana oblíqua");
        assert_eq!(quality, Quality::Exact);
        // Spread over lines and in another case, it is the same text.
        let (from, to, quality) = folded
            .find(&quote("RESSACA. OLHOS", "olhos de ", " de cigana"), None)
            .unwrap();
        assert_eq!(&TEXT[from..to], "ressaca.\n\n   Olhos");
        assert_eq!(quality, Quality::Exact);
    }

    #[test]
    fn the_context_picks_among_repeats_and_the_hint_among_ties() {
        let folded = Folded::new(TEXT);
        // Three "olhos de ressaca": the context says the one after "sim, ".
        let (from, _, quality) = folded
            .find(&quote("olhos de ressaca", "ressaca, sim, ", "."), None)
            .unwrap();
        assert_eq!(&TEXT[from - 5..from], "sim, ");
        assert_eq!(quality, Quality::Exact);
        // No context: the one nearest where it was, and only approximately.
        let second = TEXT.find("Olhos de ressaca,").unwrap();
        let (from, _, quality) = folded
            .find(&quote("olhos de ressaca", "", ""), Some(second + 2))
            .unwrap();
        assert_eq!(from, second);
        assert_eq!(quality, Quality::Approximate);
    }

    #[test]
    fn a_light_edit_is_found_approximately_and_a_rewrite_not_at_all() {
        let edited =
            "Capitu tinha olhos de ressaca. Olhos de cigana obliqua e muito dissimulada. Fim.";
        let folded = Folded::new(edited);
        let (from, to, quality) = folded
            .find(
                &quote(
                    "Olhos de cigana oblíqua e dissimulada",
                    "ressaca. ",
                    ". Fim",
                ),
                None,
            )
            .unwrap();
        assert_eq!(
            &edited[from..to],
            "Olhos de cigana obliqua e muito dissimulada"
        );
        assert_eq!(quality, Quality::Approximate);
        let rewritten =
            Folded::new("Um texto que já não fala de olhos nem de ciganas em parte alguma.");
        assert_eq!(
            rewritten.find(
                &quote("Olhos de cigana oblíqua e dissimulada", "", ""),
                None
            ),
            None
        );
        assert_eq!(Folded::new("abc").find(&quote("", "", ""), None), None);
    }

    #[test]
    fn a_removed_passage_is_not_taken_for_one_that_only_starts_alike() {
        let original = "Capítulo 1, parágrafo 9: o vento leva a carta. Capítulo 1, parágrafo 10: a pedra guarda o silêncio da casa antiga. Capítulo 1, parágrafo 11: o mar devolve a voz.";
        let removed = "Capítulo 1, parágrafo 10: a pedra guarda o silêncio da casa antiga.";
        let start = original.find(removed).unwrap();
        let quote = Quote {
            exact: removed.to_owned(),
            prefix: tail_chars(&original[..start], CONTEXT_CHARS),
            suffix: head_chars(&original[start + removed.len()..], CONTEXT_CHARS),
        };
        let without = original.replace(&format!("{removed} "), "");
        assert_eq!(Folded::new(&without).find(&quote, None), None);
        // With a word changed, it is found again, approximately.
        let edited = original.replace("guarda o silêncio", "guarda a lembrança");
        let (from, to, quality) = Folded::new(&edited).find(&quote, None).unwrap();
        assert_eq!(quality, Quality::Approximate);
        assert!(edited[from..to].contains("lembrança"));
    }

    #[test]
    fn similarity_is_one_less_the_edit_distance() {
        assert_eq!(similarity("abc", "abc"), 1.0);
        assert!((similarity("kitten", "sitting") - (1.0 - 3.0 / 7.0)).abs() < 1e-12);
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abc", ""), 0.0);
    }
}
