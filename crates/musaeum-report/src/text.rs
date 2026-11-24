//! Text from books and notes made fit to be set: line breaks of every kind made one, invisible
//! characters dropped, and a Latin letter followed by a combining accent made the one precomposed
//! character, which every font draws well, as `e` and U+0301 become `é`.

/// A letter, a combining mark after it, and the character they make, for the Latin letters of
/// Latin-1 and Latin Extended-A.
#[rustfmt::skip]
const COMPOSED: &[(char, char, char)] = &[
    ('A', '\u{300}', 'À'), ('A', '\u{301}', 'Á'), ('A', '\u{302}', 'Â'), ('A', '\u{303}', 'Ã'),
    ('A', '\u{308}', 'Ä'), ('A', '\u{30A}', 'Å'), ('A', '\u{304}', 'Ā'), ('A', '\u{306}', 'Ă'),
    ('A', '\u{328}', 'Ą'), ('C', '\u{327}', 'Ç'), ('C', '\u{301}', 'Ć'), ('C', '\u{302}', 'Ĉ'),
    ('C', '\u{307}', 'Ċ'), ('C', '\u{30C}', 'Č'), ('D', '\u{30C}', 'Ď'), ('E', '\u{300}', 'È'),
    ('E', '\u{301}', 'É'), ('E', '\u{302}', 'Ê'), ('E', '\u{308}', 'Ë'), ('E', '\u{304}', 'Ē'),
    ('E', '\u{306}', 'Ĕ'), ('E', '\u{307}', 'Ė'), ('E', '\u{328}', 'Ę'), ('E', '\u{30C}', 'Ě'),
    ('G', '\u{302}', 'Ĝ'), ('G', '\u{306}', 'Ğ'), ('G', '\u{307}', 'Ġ'), ('G', '\u{327}', 'Ģ'),
    ('H', '\u{302}', 'Ĥ'), ('I', '\u{300}', 'Ì'), ('I', '\u{301}', 'Í'), ('I', '\u{302}', 'Î'),
    ('I', '\u{308}', 'Ï'), ('I', '\u{303}', 'Ĩ'), ('I', '\u{304}', 'Ī'), ('I', '\u{306}', 'Ĭ'),
    ('I', '\u{328}', 'Į'), ('I', '\u{307}', 'İ'), ('J', '\u{302}', 'Ĵ'), ('K', '\u{327}', 'Ķ'),
    ('L', '\u{301}', 'Ĺ'), ('L', '\u{327}', 'Ļ'), ('L', '\u{30C}', 'Ľ'), ('N', '\u{303}', 'Ñ'),
    ('N', '\u{301}', 'Ń'), ('N', '\u{327}', 'Ņ'), ('N', '\u{30C}', 'Ň'), ('O', '\u{300}', 'Ò'),
    ('O', '\u{301}', 'Ó'), ('O', '\u{302}', 'Ô'), ('O', '\u{303}', 'Õ'), ('O', '\u{308}', 'Ö'),
    ('O', '\u{304}', 'Ō'), ('O', '\u{306}', 'Ŏ'), ('O', '\u{30B}', 'Ő'), ('R', '\u{301}', 'Ŕ'),
    ('R', '\u{327}', 'Ŗ'), ('R', '\u{30C}', 'Ř'), ('S', '\u{301}', 'Ś'), ('S', '\u{302}', 'Ŝ'),
    ('S', '\u{327}', 'Ş'), ('S', '\u{30C}', 'Š'), ('T', '\u{327}', 'Ţ'), ('T', '\u{30C}', 'Ť'),
    ('U', '\u{300}', 'Ù'), ('U', '\u{301}', 'Ú'), ('U', '\u{302}', 'Û'), ('U', '\u{308}', 'Ü'),
    ('U', '\u{303}', 'Ũ'), ('U', '\u{304}', 'Ū'), ('U', '\u{306}', 'Ŭ'), ('U', '\u{30A}', 'Ů'),
    ('U', '\u{30B}', 'Ű'), ('U', '\u{328}', 'Ų'), ('W', '\u{302}', 'Ŵ'), ('Y', '\u{301}', 'Ý'),
    ('Y', '\u{302}', 'Ŷ'), ('Y', '\u{308}', 'Ÿ'), ('Z', '\u{301}', 'Ź'), ('Z', '\u{307}', 'Ż'),
    ('Z', '\u{30C}', 'Ž'),
    ('a', '\u{300}', 'à'), ('a', '\u{301}', 'á'), ('a', '\u{302}', 'â'), ('a', '\u{303}', 'ã'),
    ('a', '\u{308}', 'ä'), ('a', '\u{30A}', 'å'), ('a', '\u{304}', 'ā'), ('a', '\u{306}', 'ă'),
    ('a', '\u{328}', 'ą'), ('c', '\u{327}', 'ç'), ('c', '\u{301}', 'ć'), ('c', '\u{302}', 'ĉ'),
    ('c', '\u{307}', 'ċ'), ('c', '\u{30C}', 'č'), ('d', '\u{30C}', 'ď'), ('e', '\u{300}', 'è'),
    ('e', '\u{301}', 'é'), ('e', '\u{302}', 'ê'), ('e', '\u{308}', 'ë'), ('e', '\u{304}', 'ē'),
    ('e', '\u{306}', 'ĕ'), ('e', '\u{307}', 'ė'), ('e', '\u{328}', 'ę'), ('e', '\u{30C}', 'ě'),
    ('g', '\u{302}', 'ĝ'), ('g', '\u{306}', 'ğ'), ('g', '\u{307}', 'ġ'), ('g', '\u{327}', 'ģ'),
    ('h', '\u{302}', 'ĥ'), ('i', '\u{300}', 'ì'), ('i', '\u{301}', 'í'), ('i', '\u{302}', 'î'),
    ('i', '\u{308}', 'ï'), ('i', '\u{303}', 'ĩ'), ('i', '\u{304}', 'ī'), ('i', '\u{306}', 'ĭ'),
    ('i', '\u{328}', 'į'), ('j', '\u{302}', 'ĵ'), ('k', '\u{327}', 'ķ'), ('l', '\u{301}', 'ĺ'),
    ('l', '\u{327}', 'ļ'), ('l', '\u{30C}', 'ľ'), ('n', '\u{303}', 'ñ'), ('n', '\u{301}', 'ń'),
    ('n', '\u{327}', 'ņ'), ('n', '\u{30C}', 'ň'), ('o', '\u{300}', 'ò'), ('o', '\u{301}', 'ó'),
    ('o', '\u{302}', 'ô'), ('o', '\u{303}', 'õ'), ('o', '\u{308}', 'ö'), ('o', '\u{304}', 'ō'),
    ('o', '\u{306}', 'ŏ'), ('o', '\u{30B}', 'ő'), ('r', '\u{301}', 'ŕ'), ('r', '\u{327}', 'ŗ'),
    ('r', '\u{30C}', 'ř'), ('s', '\u{301}', 'ś'), ('s', '\u{302}', 'ŝ'), ('s', '\u{327}', 'ş'),
    ('s', '\u{30C}', 'š'), ('t', '\u{327}', 'ţ'), ('t', '\u{30C}', 'ť'), ('u', '\u{300}', 'ù'),
    ('u', '\u{301}', 'ú'), ('u', '\u{302}', 'û'), ('u', '\u{308}', 'ü'), ('u', '\u{303}', 'ũ'),
    ('u', '\u{304}', 'ū'), ('u', '\u{306}', 'ŭ'), ('u', '\u{30A}', 'ů'), ('u', '\u{30B}', 'ű'),
    ('u', '\u{328}', 'ų'), ('w', '\u{302}', 'ŵ'), ('y', '\u{301}', 'ý'), ('y', '\u{302}', 'ŷ'),
    ('y', '\u{308}', 'ÿ'), ('z', '\u{301}', 'ź'), ('z', '\u{307}', 'ż'), ('z', '\u{30C}', 'ž'),
];

/// Whether a character is a combining mark of the block U+0300 to U+036F, drawn over the letter
/// before it.
pub fn combining(c: char) -> bool {
    ('\u{300}'..='\u{36F}').contains(&c)
}

fn compose(base: char, mark: char) -> Option<char> {
    COMPOSED
        .iter()
        .find(|(letter, accent, _)| *letter == base && *accent == mark)
        .map(|(_, _, made)| *made)
}

/// Characters that take no room and draw nothing, and are left out: soft hyphens, joiners, the
/// byte order mark, and control characters other than line breaks and tabs.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}'
    ) || (c.is_control() && !matches!(c, '\n' | '\t'))
}

/// Text made fit to be set: every kind of line break a `\n`, invisible characters out, letters and
/// their combining accents composed.
pub fn clean(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let c = match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                '\n'
            }
            '\u{2028}' | '\u{2029}' | '\u{85}' | '\u{0B}' | '\u{0C}' => '\n',
            c if invisible(c) => continue,
            c => c,
        };
        if combining(c)
            && let Some(last) = out.pop()
        {
            match compose(last, c) {
                Some(made) => out.push(made),
                None => {
                    out.push(last);
                    out.push(c);
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// White space folded to single spaces and trimmed, line breaks with it: a label, a title.
pub fn fold_spaces(text: &str) -> String {
    clean(text).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accents_typed_apart_are_composed() {
        assert_eq!(clean("Ac\u{327}a\u{303}o e\u{301}"), "Ação é");
        assert_eq!(
            clean("x\u{301}"),
            "x\u{301}",
            "no precomposed letter: left as it is"
        );
        assert_eq!(
            clean("\u{301}a"),
            "\u{301}a",
            "a mark with nothing before it"
        );
    }

    #[test]
    fn breaks_become_one_and_the_invisible_go() {
        assert_eq!(clean("a\r\nb\rc\u{2028}d"), "a\nb\nc\nd");
        assert_eq!(clean("in\u{AD}vi\u{200B}sí\u{FEFF}vel\u{7}"), "invisível");
        assert_eq!(
            fold_spaces("  Capítulo\n 1 \t Introdução "),
            "Capítulo 1 Introdução"
        );
    }
}
