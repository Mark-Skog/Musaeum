//! The order a report follows: the book's. Annotations by where they start, under the deepest entry
//! of the table of contents at or before them; notes about the whole book first; those that could
//! not be found on the book as it is now last, apart, with the place they were last seen.

use crate::{Found, Heading, Item, WorkReport};

/// A stretch of a work under one entry of its contents, or before any.
#[derive(Debug)]
pub struct Section<'a> {
    /// The entry and the entries above it, from the top level down. Empty before the first one.
    pub path: Vec<&'a Heading>,
    pub items: Vec<&'a Item>,
}

/// A work's annotations, in the order a report shows them.
#[derive(Debug)]
pub struct Ordered<'a> {
    /// Notes about the whole book.
    pub whole: Vec<&'a Item>,
    pub sections: Vec<Section<'a>>,
    /// Not found on the book as it is now.
    pub orphans: Vec<&'a Item>,
}

/// How a work's annotations are ordered and grouped.
pub fn order(work: &WorkReport) -> Ordered<'_> {
    let key = |item: &Item| {
        (
            item.at.or(item.progression).unwrap_or(f64::INFINITY),
            item.created_at,
            item.uuid.clone(),
        )
    };
    let by_place = |a: &&Item, b: &&Item| {
        let (x, y) = (key(a), key(b));
        x.0.total_cmp(&y.0)
            .then(x.1.cmp(&y.1))
            .then_with(|| x.2.cmp(&y.2))
    };
    let mut whole = Vec::new();
    let mut placed = Vec::new();
    let mut orphans = Vec::new();
    for item in &work.items {
        if item.found == Found::Orphan {
            orphans.push(item);
        } else if item.at.is_none() {
            whole.push(item);
        } else {
            placed.push(item);
        }
    }
    whole.sort_by(by_place);
    placed.sort_by(by_place);
    orphans.sort_by(by_place);

    // The entries with a place, in the order of the contents; each one's parent is the nearest
    // entry before it one level up.
    let entries: Vec<&Heading> = work
        .contents
        .iter()
        .filter(|heading| heading.at.is_finite() && !heading.label.trim().is_empty())
        .collect();
    let mut parent: Vec<Option<usize>> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        parent.push(
            (0..index)
                .rev()
                .find(|&before| entries[before].depth < entry.depth),
        );
    }
    // By where they start; among those that start at the same place, the last in the contents,
    // the deepest, is the one an annotation is under.
    let mut by_start: Vec<usize> = (0..entries.len()).collect();
    by_start.sort_by(|&a, &b| entries[a].at.total_cmp(&entries[b].at).then(a.cmp(&b)));
    let under = |at: f64| -> Option<usize> {
        let count = by_start.partition_point(|&index| entries[index].at <= at + 1e-9);
        count.checked_sub(1).map(|last| by_start[last])
    };
    let path_of = |index: Option<usize>| -> Vec<&Heading> {
        let mut path = Vec::new();
        let mut at = index;
        while let Some(current) = at {
            path.push(entries[current]);
            at = parent[current];
        }
        path.reverse();
        path
    };

    let mut sections: Vec<Section<'_>> = Vec::new();
    let mut current: Option<Option<usize>> = None;
    for item in placed {
        let entry = under(item.at.unwrap_or(0.0));
        if current != Some(entry) {
            sections.push(Section {
                path: path_of(entry),
                items: Vec::new(),
            });
            current = Some(entry);
        }
        if let Some(section) = sections.last_mut() {
            section.items.push(item);
        }
    }
    Ordered {
        whole,
        sections,
        orphans,
    }
}

/// How many of each kind, in words: `7 realces, 3 sublinhados e 2 notas`.
pub fn tally(items: &[Item]) -> String {
    let count = |label: &str, singular: &str, plural: &str| -> Option<String> {
        let n = items.iter().filter(|item| item.label() == label).count();
        (n > 0).then(|| format!("{n} {}", if n == 1 { singular } else { plural }))
    };
    let parts: Vec<String> = [
        count("Realce", "realce", "realces"),
        count("Sublinhado", "sublinhado", "sublinhados"),
        count("Nota", "nota", "notas"),
        count("Caixa", "caixa", "caixas"),
        count("Traço", "traço", "traços"),
        count("Nota fixada", "nota fixada", "notas fixadas"),
    ]
    .into_iter()
    .flatten()
    .collect();
    match parts.as_slice() {
        [] => "nenhuma anotação".to_owned(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} e {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Kind, Layout, Mark};

    fn item(uuid: &str, at: Option<f64>, found: Found) -> Item {
        Item {
            uuid: uuid.into(),
            kind: Kind::Highlight,
            mark: Mark::Text,
            color: "yellow".into(),
            quote: Some(uuid.into()),
            note: String::new(),
            at,
            page: at.map(|at| at as u32),
            progression: at.map(|at| at / 100.0),
            found,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn heading(label: &str, depth: u32, at: f64) -> Heading {
        Heading {
            label: label.into(),
            depth,
            at,
        }
    }

    #[test]
    fn annotations_go_under_the_deepest_entry_before_them_in_reading_order() {
        let work = WorkReport {
            title: "Livro".into(),
            creators: Vec::new(),
            series: None,
            series_position: None,
            year: None,
            language: None,
            identifiers: Vec::new(),
            format: "pdf".into(),
            layout: Layout::Fixed,
            pages: Some(40),
            contents: vec![
                heading("Parte I", 0, 2.0),
                heading("Capítulo 1", 1, 2.0),
                heading("Capítulo 2", 1, 10.0),
                heading("Parte II", 0, 20.0),
                heading("Sem lugar", 1, f64::NAN),
                heading("Capítulo 3", 1, 21.0),
            ],
            items: vec![
                item("c3", Some(25.5), Found::Exact),
                item("antes", Some(0.4), Found::Exact),
                item("c1b", Some(3.7), Found::Approximate),
                item("c1a", Some(2.1), Found::Exact),
                item("órfã", Some(5.0), Found::Orphan),
                item("todo", None, Found::Exact),
                item("p2", Some(20.2), Found::Exact),
                item("c2", Some(10.0), Found::Exact),
            ],
        };
        let ordered = order(&work);
        let shown: Vec<(Vec<&str>, Vec<&str>)> = ordered
            .sections
            .iter()
            .map(|section| {
                (
                    section.path.iter().map(|h| h.label.as_str()).collect(),
                    section.items.iter().map(|i| i.uuid.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                (vec![], vec!["antes"]),
                (vec!["Parte I", "Capítulo 1"], vec!["c1a", "c1b"]),
                (vec!["Parte I", "Capítulo 2"], vec!["c2"]),
                (vec!["Parte II"], vec!["p2"]),
                (vec!["Parte II", "Capítulo 3"], vec!["c3"]),
            ]
        );
        assert_eq!(ordered.whole.len(), 1);
        assert_eq!(ordered.orphans[0].uuid, "órfã");
        assert_eq!(tally(&work.items), "8 realces");
    }
}
