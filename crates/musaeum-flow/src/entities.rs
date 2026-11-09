//! HTML's named character references, from the table the HTML parser uses.

use html5ever::data::NAMED_ENTITIES;

/// The text `&name;` stands for in HTML, if it stands for anything.
pub fn named(name: &str) -> Option<String> {
    let mut key = String::with_capacity(name.len() + 1);
    key.push_str(name);
    key.push(';');
    let &(first, second) = NAMED_ENTITIES.get(key.as_str())?;
    let mut text = String::new();
    text.push(char::from_u32(first)?);
    if second != 0 {
        text.push(char::from_u32(second)?);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    #[test]
    fn common_entities_resolve() {
        assert_eq!(super::named("nbsp").as_deref(), Some("\u{a0}"));
        assert_eq!(super::named("eacute").as_deref(), Some("é"));
        assert_eq!(
            super::named("NotEqualTilde").as_deref(),
            Some("\u{2242}\u{338}")
        );
        assert_eq!(super::named("bogus"), None);
    }
}
