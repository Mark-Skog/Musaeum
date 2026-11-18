//! Orders a person keeps by hand: siblings in the category tree, books in a collection. Each item
//! holds a key, and the items sort by their keys as text; between any two keys there is always
//! another, so moving an item rewrites its own key and nothing else.
//!
//! A key is a fraction in base 62 written with the digits `0-9A-Za-z`, which sort as text in the
//! order of their values: `"V"` is 31/62, `"V5"` is 31/62 + 5/62². No key ends in `0`, so there is
//! always room after the last digit. A key between two others takes the middle digit where they
//! first differ by more than one, so it is one digit longer at most; one after the last key adds
//! one to the first digit that can take it, so a list appended to grows one digit longer every 61
//! items.

/// The digits, in the order of their values and of their bytes.
const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = 62;

/// What a key that is not one is.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("an order key is made of the digits 0-9, A-Z and a-z, and does not end in 0: {0:?}")]
    Malformed(String),
    #[error("no key lies between {0:?} and {1:?}: the first is not before the second")]
    NotBefore(String, String),
}

fn value(digit: u8) -> Option<usize> {
    match digit {
        b'0'..=b'9' => Some(usize::from(digit - b'0')),
        b'A'..=b'Z' => Some(usize::from(digit - b'A') + 10),
        b'a'..=b'z' => Some(usize::from(digit - b'a') + 36),
        _ => None,
    }
}

/// Whether `key` is a well-formed key.
pub fn is_key(key: &str) -> bool {
    !key.is_empty() && !key.ends_with('0') && key.bytes().all(|digit| value(digit).is_some())
}

fn checked(key: &str) -> Result<&[u8], KeyError> {
    if is_key(key) {
        Ok(key.as_bytes())
    } else {
        Err(KeyError::Malformed(key.to_owned()))
    }
}

fn text(digits: Vec<u8>) -> String {
    String::from_utf8(digits).expect("digits are ASCII")
}

/// A key after `before` and before `after`; `None` is the start or the end of the list.
pub fn between(before: Option<&str>, after: Option<&str>) -> Result<String, KeyError> {
    let low = before.map(checked).transpose()?.unwrap_or(&[]);
    match after {
        None => Ok(text(after_key(low))),
        Some(after) => {
            let high = checked(after)?;
            if low >= high {
                return Err(KeyError::NotBefore(
                    before.unwrap_or_default().to_owned(),
                    after.to_owned(),
                ));
            }
            Ok(text(middle(low, high)))
        }
    }
}

/// `n` keys in order, for a list made at once.
pub fn spread(n: usize) -> Vec<String> {
    let mut keys = Vec::with_capacity(n);
    let mut last: Option<String> = None;
    for _ in 0..n {
        let key = text(after_key(last.as_deref().map_or(&[][..], str::as_bytes)));
        keys.push(key.clone());
        last = Some(key);
    }
    keys
}

/// The first key of a list.
pub fn first() -> String {
    text(after_key(&[]))
}

/// A key after `low`, with nothing after it.
fn after_key(low: &[u8]) -> Vec<u8> {
    if low.is_empty() {
        // The middle, so a list may grow both ways.
        return vec![DIGITS[BASE / 2]];
    }
    let mut out = Vec::with_capacity(low.len() + 1);
    for &digit in low {
        let at = value(digit).expect("checked");
        if at + 1 < BASE {
            out.push(DIGITS[at + 1]);
            return out;
        }
        out.push(digit);
    }
    // Every digit is the last one: one more digit, the smallest a key may end in, so the next 60
    // keys after it are one digit long too.
    out.push(DIGITS[1]);
    out
}

/// A key strictly between `low` and `high`, `low < high`; `low` may be empty, the start.
fn middle(low: &[u8], high: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(high.len().max(low.len()) + 1);
    let mut at = 0;
    loop {
        let a = low
            .get(at)
            .map_or(0, |&digit| value(digit).expect("checked"));
        // Past the end of `high`, `low` is already less: nothing above bounds it there.
        let b = high
            .get(at)
            .map_or(BASE, |&digit| value(digit).expect("checked"));
        if a == b {
            out.push(DIGITS[a]);
            at += 1;
            continue;
        }
        if b - a > 1 {
            out.push(DIGITS[(a + b) / 2]);
            return out;
        }
        // Next to each other: keep `low`'s digit, and go above the rest of `low`, which `high`
        // no longer bounds.
        out.push(DIGITS[a]);
        at += 1;
        loop {
            let a = low
                .get(at)
                .map_or(0, |&digit| value(digit).expect("checked"));
            if a + 1 < BASE {
                out.push(DIGITS[(a + BASE).div_ceil(2)]);
                return out;
            }
            out.push(DIGITS[a]);
            at += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordered(keys: &[String]) -> bool {
        keys.windows(2).all(|pair| pair[0] < pair[1]) && keys.iter().all(|key| is_key(key))
    }

    #[test]
    fn keys_fall_between_their_neighbours() {
        assert_eq!(first(), "V");
        assert_eq!(between(Some("V"), None).unwrap(), "W");
        assert_eq!(between(None, Some("V")).unwrap(), "F");
        assert_eq!(between(Some("V"), Some("X")).unwrap(), "W");
        // Next to each other: one digit more.
        let key = between(Some("V"), Some("W")).unwrap();
        assert!(
            "V" < key.as_str() && key.as_str() < "W" && is_key(&key),
            "{key}"
        );
        // Past the last digit of the list.
        assert_eq!(between(Some("z"), None).unwrap(), "z1");
        assert_eq!(between(Some("zz"), None).unwrap(), "zz1");
        let key = between(None, Some("1")).unwrap();
        assert!(key.as_str() < "1" && is_key(&key), "{key}");
        let key = between(None, Some("01")).unwrap();
        assert!(key.as_str() < "01" && is_key(&key), "{key}");
    }

    #[test]
    fn what_is_not_a_key_or_not_in_order_is_refused() {
        assert!(matches!(
            between(Some("V0"), None),
            Err(KeyError::Malformed(_))
        ));
        assert!(matches!(
            between(Some("V-"), None),
            Err(KeyError::Malformed(_))
        ));
        assert!(matches!(
            between(Some("W"), Some("V")),
            Err(KeyError::NotBefore(..))
        ));
        assert!(matches!(
            between(Some("V"), Some("V")),
            Err(KeyError::NotBefore(..))
        ));
    }

    #[test]
    fn a_list_stays_in_order_whatever_is_inserted_where() {
        // A fixed sequence of inserts at the start, the end and in between, as a person reorders.
        let mut keys = vec![first()];
        let mut seed = 0x2545_f491_u64;
        for _ in 0..3000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let at = (seed % (keys.len() as u64 + 1)) as usize;
            let before = at.checked_sub(1).map(|i| keys[i].as_str());
            let after = keys.get(at).map(String::as_str);
            let key = between(before, after).unwrap();
            keys.insert(at, key);
        }
        assert!(ordered(&keys));
        let longest = keys.iter().map(String::len).max().unwrap();
        assert!(longest <= 40, "{longest}");
    }

    #[test]
    fn a_list_appended_to_grows_slowly() {
        let keys = spread(1000);
        assert!(ordered(&keys));
        assert!(
            keys.iter().map(String::len).max().unwrap() <= 18,
            "{:?}",
            keys.last()
        );
        // Always inserting right after the first item, the worst case for the middle.
        let mut keys = spread(2);
        for _ in 0..200 {
            let key = between(Some(&keys[0]), Some(&keys[1])).unwrap();
            keys.insert(1, key);
        }
        assert!(ordered(&keys));
    }
}
