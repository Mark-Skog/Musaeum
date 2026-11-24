//! The languages Musaeum speaks, for what its Rust code writes for a person: the reasons a file is
//! refused, the labels of a report, the subject of an annotation in a PDF, a dialog's title.
//!
//! Every message is written once in `catalog.rs`, in Brazilian Portuguese and in English side by
//! side: a message without one of them does not compile. A message names the values it takes in
//! braces, `{path}`, the same names in both languages, which the tests hold.
//!
//! The language is the person's, from the preferences, and the same for the whole app: the host
//! sets it when the app starts and whenever it changes ([`set_locale`]). What has to come out the
//! same whatever the app's language is, such as a report written for a test, asks for its own
//! ([`text_in`]).
//!
//! What is not written for a person stays in English and out of here: a worker's protocol, an
//! invariant broken inside the program, a system's own error. A person sees it, if at all, after a
//! sentence of their language that says what could not be done.

mod catalog;

use std::fmt::{self, Display, Write as _};
use std::sync::atomic::{AtomicU8, Ordering};

pub use catalog::T;

/// A language of the interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Locale {
    /// Brazilian Portuguese, the first.
    #[default]
    PtBr,
    English,
}

impl Locale {
    /// Every language, in the order they are offered.
    pub const ALL: [Locale; 2] = [Locale::PtBr, Locale::English];

    /// Its BCP 47 tag, as the interface and the preferences name it.
    pub fn tag(self) -> &'static str {
        match self {
            Locale::PtBr => "pt-BR",
            Locale::English => "en",
        }
    }

    /// The language a tag names, if it is one of these.
    pub fn from_tag(tag: &str) -> Option<Locale> {
        Locale::ALL
            .into_iter()
            .find(|locale| locale.tag().eq_ignore_ascii_case(tag))
    }

    fn index(self) -> u8 {
        match self {
            Locale::PtBr => 0,
            Locale::English => 1,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Makes `locale` the language of everything written from now on.
pub fn set_locale(locale: Locale) {
    CURRENT.store(locale.index(), Ordering::Relaxed);
}

/// The language messages are written in now.
pub fn locale() -> Locale {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Locale::English,
        _ => Locale::PtBr,
    }
}

/// A message in the current language, with its values put in.
pub fn text(message: T, values: &[(&str, &dyn Display)]) -> String {
    text_in(locale(), message, values)
}

/// A message in `locale`, with its values put in.
pub fn text_in(locale: Locale, message: T, values: &[(&str, &dyn Display)]) -> String {
    fill(message.template(locale), values)
}

/// One of two messages by a count: `one` for exactly one, `other` for any other number, as the
/// interface counts too.
pub fn count_in(
    locale: Locale,
    count: u64,
    one: T,
    other: T,
    values: &[(&str, &dyn Display)],
) -> String {
    text_in(locale, if count == 1 { one } else { other }, values)
}

/// [`count_in`], in the current language.
pub fn count(count: u64, one: T, other: T, values: &[(&str, &dyn Display)]) -> String {
    count_in(locale(), count, one, other, values)
}

/// A message in the current language: `tr!(Key)` or `tr!(Key, path = shown, count = n)`.
#[macro_export]
macro_rules! tr {
    ($key:ident $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::text(
            $crate::T::$key,
            &[$((stringify!($name), &$value as &dyn ::core::fmt::Display)),*],
        )
    };
}

/// A message in a language given: `tr_in!(locale, Key, name = value)`.
#[macro_export]
macro_rules! tr_in {
    ($locale:expr, $key:ident $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::text_in(
            $locale,
            $crate::T::$key,
            &[$((stringify!($name), &$value as &dyn ::core::fmt::Display)),*],
        )
    };
}

/// Puts each `{name}` of `template` in from `values`. Braces around anything that is not a name
/// stay as they are, and a name without a value stays as it was written, so that a missing value
/// shows instead of vanishing (and fails a debug build).
pub fn fill(template: &str, values: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_end];
        if !is_name(name) || !after[name_end..].starts_with('}') {
            out.push('{');
            rest = after;
            continue;
        }
        match values.iter().find(|(key, _)| *key == name) {
            Some((_, value)) => {
                let _ = write!(out, "{value}");
            }
            None => {
                debug_assert!(false, "no value for {{{name}}} in {template:?}");
                out.push('{');
                out.push_str(name);
                out.push('}');
            }
        }
        rest = &after[name_end + 1..];
    }
    out.push_str(rest);
    out
}

/// The names a template takes, in the order they first appear.
pub fn placeholders(template: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let name_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let name = &after[..name_end];
        if is_name(name) && after[name_end..].starts_with('}') && !names.contains(&name) {
            names.push(name);
        }
        rest = after;
    }
    names
}

/// A value's name: a letter or _, then letters, digits or _.
fn is_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
}

impl fmt::Display for Locale {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_go_where_their_names_are_and_other_braces_stay() {
        assert_eq!(
            fill(
                "{path} tem {count} livros",
                &[("path", &"C:\\a"), ("count", &3)]
            ),
            "C:\\a tem 3 livros"
        );
        assert_eq!(fill("{ x } e {} e {1a}", &[]), "{ x } e {} e {1a}");
        assert_eq!(fill("sem nada", &[]), "sem nada");
        assert_eq!(fill("{a}{a}", &[("a", &"x")]), "xx");
    }

    #[test]
    fn every_message_takes_the_same_values_in_both_languages() {
        for message in T::ALL {
            let mut pt = placeholders(message.template(Locale::PtBr));
            let mut en = placeholders(message.template(Locale::English));
            pt.sort_unstable();
            en.sort_unstable();
            assert_eq!(pt, en, "{message:?}");
        }
    }

    #[test]
    fn every_message_is_written_in_both_languages() {
        for message in T::ALL {
            for locale in Locale::ALL {
                let template = message.template(locale);
                assert!(!template.trim().is_empty(), "{message:?} in {locale}");
                assert_eq!(template, template.trim(), "{message:?} in {locale}");
            }
            // English is not Portuguese left untranslated: no Portuguese letters, unless the
            // message is a name.
            let english = message.template(Locale::English);
            assert!(
                !english.contains(['ã', 'õ', 'ç', 'á', 'é', 'í', 'ó', 'ú', 'â', 'ê', 'ô', 'à']),
                "{message:?} in English: {english}"
            );
        }
    }

    #[test]
    fn the_language_is_the_one_set_last() {
        let before = locale();
        set_locale(Locale::English);
        assert_eq!(locale(), Locale::English);
        assert_eq!(Locale::from_tag("PT-br"), Some(Locale::PtBr));
        assert_eq!(Locale::from_tag("es"), None);
        set_locale(before);
    }
}
