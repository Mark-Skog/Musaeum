//! What the command line asks for.

use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: musaeum [--library DIR] [--json] COMMAND

Folders
  root add PATH [--exclude PATTERN]...   Start following a folder
  root list                              The folders being followed
  root remove ID                         Stop following one. Nothing is lost
  root relocate ID PATH                  Say where a folder has moved to

Scanning
  reconcile [--root ID] [--no-hash]      Compare the disk with the library
  hash                                   Calculate the hashes still waiting
  watch [--interval SECONDS]             Keep up with the disk until stopped

Looking
  status                                 How many of each thing there are
  list [--state STATE] [--limit N]       Files: present, absent, cloud_only, unreadable
  inspect PATH | doc:ID | work:ID        What the library knows about something
  revisions PATH | doc:ID                A document's versions and where each one is
  search QUERY [--limit N] [--keystrokes]
                                         Find works; --keystrokes times each letter typed
  suggestions [--all]                    What the library noticed and left for you

Deciding
  suggestions accept ID                  They are the same document
  suggestions dismiss ID                 They are not
  edit PATH [--title T] [--rating N] [--status S] [--series S] [--creators A;B]
  note add PATH TEXT                     Add a note to a document
  reimport PATH                          Read a file's metadata again. Your edits stay

The disk, always through the journal
  move SOURCE TARGET                     Move or rename a file or folder, never overwriting
  recycle PATH                           Send a file to the Recycle Bin
  undo [OPERATION]                       Take back an operation, by default the last one
  journal [--limit N]                    What has been done

Backups
  backup now                             Copy the library now
  backup list                            The copies there are
  backup restore FILE                    Go back to a copy, keeping the current library aside

Maintenance
  rebuild-search                         Write the search indexes again

The library lives in %LOCALAPPDATA%\\Musaeum unless --library says otherwise.";

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct Invocation {
    pub library: Option<PathBuf>,
    pub json: bool,
    pub command: Command,
}

/// One thing to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    RootAdd {
        path: PathBuf,
        exclude: Vec<String>,
    },
    RootList,
    RootRemove {
        id: i64,
    },
    RootRelocate {
        id: i64,
        path: PathBuf,
    },
    Reconcile {
        root: Option<i64>,
        hash: bool,
    },
    Hash,
    Watch {
        interval: Option<u64>,
    },
    Status,
    List {
        state: Option<String>,
        limit: usize,
    },
    Inspect {
        target: Target,
    },
    Revisions {
        target: Target,
    },
    Search {
        query: String,
        limit: usize,
        keystrokes: bool,
    },
    Suggestions {
        all: bool,
    },
    SuggestionAccept {
        id: i64,
    },
    SuggestionDismiss {
        id: i64,
    },
    Edit {
        path: PathBuf,
        edit: Edit,
    },
    NoteAdd {
        path: PathBuf,
        text: String,
    },
    Reimport {
        path: PathBuf,
    },
    Move {
        source: PathBuf,
        target: PathBuf,
    },
    Recycle {
        path: PathBuf,
    },
    Undo {
        operation: Option<i64>,
    },
    Journal {
        limit: usize,
    },
    BackupNow,
    BackupList,
    BackupRestore {
        file: PathBuf,
    },
    RebuildSearch,
}

/// What `inspect` and `revisions` can be pointed at.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    Path(PathBuf),
    Document(i64),
    Work(i64),
}

/// What `edit` can change.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub title: Option<String>,
    pub rating: Option<i64>,
    pub status: Option<String>,
    pub series: Option<String>,
    pub creators: Option<Vec<String>>,
}

/// Why there is nothing to run.
#[derive(Debug, PartialEq, Eq)]
pub enum ArgsError {
    Help,
    Invalid(String),
}

struct Arguments {
    items: std::vec::IntoIter<OsString>,
}

impl Arguments {
    fn next_os(&mut self) -> Option<OsString> {
        self.items.next()
    }

    fn next_text(&mut self) -> Option<Result<String, ArgsError>> {
        self.items.next().map(|item| {
            item.into_string()
                .map_err(|_| ArgsError::Invalid("an argument is not valid Unicode".to_owned()))
        })
    }

    fn path(&mut self, what: &str) -> Result<PathBuf, ArgsError> {
        self.next_os()
            .map(PathBuf::from)
            .ok_or_else(|| ArgsError::Invalid(format!("{what} is missing")))
    }

    fn text(&mut self, what: &str) -> Result<String, ArgsError> {
        self.next_text()
            .ok_or_else(|| ArgsError::Invalid(format!("{what} is missing")))?
    }

    fn number<T: std::str::FromStr>(&mut self, what: &str) -> Result<T, ArgsError> {
        let text = self.text(what)?;
        text.parse().map_err(|_| {
            ArgsError::Invalid(format!("{what} has to be a number, and {text} is not"))
        })
    }

    /// Goes through what is left, which has to be options.
    fn options(
        &mut self,
        mut handle: impl FnMut(&str, &mut Self) -> Result<(), ArgsError>,
    ) -> Result<(), ArgsError> {
        while let Some(item) = self.next_text() {
            let item = item?;
            handle(&item, self)?;
        }
        Ok(())
    }
}

fn unknown(option: &str) -> ArgsError {
    ArgsError::Invalid(format!("{option} is not an option here"))
}

fn target_of(item: OsString) -> Result<Target, ArgsError> {
    if let Some(text) = item.to_str() {
        if let Some(id) = text.strip_prefix("doc:") {
            let id = id
                .parse()
                .map_err(|_| ArgsError::Invalid(format!("{id} is not a document number")))?;
            return Ok(Target::Document(id));
        }
        if let Some(id) = text.strip_prefix("work:") {
            let id = id
                .parse()
                .map_err(|_| ArgsError::Invalid(format!("{id} is not a work number")))?;
            return Ok(Target::Work(id));
        }
    }
    Ok(Target::Path(PathBuf::from(item)))
}

/// Reads the command line, without what the program was called as.
pub fn parse(arguments: Vec<OsString>) -> Result<Invocation, ArgsError> {
    let mut arguments = Arguments {
        items: arguments.into_iter(),
    };
    let mut library = None;
    let mut json = false;
    let command = loop {
        let item = arguments.next_text().ok_or(ArgsError::Help)??;
        match item.as_str() {
            "--library" => library = Some(arguments.path("the library's folder")?),
            "--json" => json = true,
            "--help" | "-h" | "help" => return Err(ArgsError::Help),
            _ => break item,
        }
    };
    let command = parse_command(&command, &mut arguments)?;
    Ok(Invocation {
        library,
        json,
        command,
    })
}

fn parse_command(command: &str, arguments: &mut Arguments) -> Result<Command, ArgsError> {
    match command {
        "root" => parse_root(arguments),
        "reconcile" => {
            let mut root = None;
            let mut hash = true;
            arguments.options(|option, rest| {
                match option {
                    "--root" => root = Some(rest.number("the root")?),
                    "--no-hash" => hash = false,
                    other => return Err(unknown(other)),
                }
                Ok(())
            })?;
            Ok(Command::Reconcile { root, hash })
        }
        "hash" => Ok(Command::Hash),
        "watch" => {
            let mut interval = None;
            arguments.options(|option, rest| {
                match option {
                    "--interval" => interval = Some(rest.number("the interval")?),
                    other => return Err(unknown(other)),
                }
                Ok(())
            })?;
            Ok(Command::Watch { interval })
        }
        "status" => Ok(Command::Status),
        "list" => {
            let mut state = None;
            let mut limit = 100;
            arguments.options(|option, rest| {
                match option {
                    "--state" => state = Some(rest.text("the state")?),
                    "--limit" => limit = rest.number("the limit")?,
                    other => return Err(unknown(other)),
                }
                Ok(())
            })?;
            Ok(Command::List { state, limit })
        }
        "inspect" => {
            let item = arguments
                .next_os()
                .ok_or_else(|| ArgsError::Invalid("what to inspect is missing".to_owned()))?;
            Ok(Command::Inspect {
                target: target_of(item)?,
            })
        }
        "revisions" => {
            let item = arguments
                .next_os()
                .ok_or_else(|| ArgsError::Invalid("which document is missing".to_owned()))?;
            Ok(Command::Revisions {
                target: target_of(item)?,
            })
        }
        "search" => {
            let query = arguments.text("what to search for")?;
            let mut limit = 20;
            let mut keystrokes = false;
            arguments.options(|option, rest| {
                match option {
                    "--limit" => limit = rest.number("the limit")?,
                    "--keystrokes" => keystrokes = true,
                    other => return Err(unknown(other)),
                }
                Ok(())
            })?;
            Ok(Command::Search {
                query,
                limit,
                keystrokes,
            })
        }
        "suggestions" => parse_suggestions(arguments),
        "edit" => parse_edit(arguments),
        "note" => match arguments.text("what to do with the note")?.as_str() {
            "add" => Ok(Command::NoteAdd {
                path: arguments.path("the file")?,
                text: arguments.text("the note")?,
            }),
            other => Err(ArgsError::Invalid(format!("note has no {other}"))),
        },
        "reimport" => Ok(Command::Reimport {
            path: arguments.path("the file")?,
        }),
        "move" => Ok(Command::Move {
            source: arguments.path("what to move")?,
            target: arguments.path("where to")?,
        }),
        "recycle" => Ok(Command::Recycle {
            path: arguments.path("the file")?,
        }),
        "undo" => {
            let operation = match arguments.next_text() {
                Some(text) => {
                    let text = text?;
                    Some(text.parse().map_err(|_| {
                        ArgsError::Invalid(format!("{text} is not an operation number"))
                    })?)
                }
                None => None,
            };
            Ok(Command::Undo { operation })
        }
        "journal" => {
            let mut limit = 20;
            arguments.options(|option, rest| {
                match option {
                    "--limit" => limit = rest.number("the limit")?,
                    other => return Err(unknown(other)),
                }
                Ok(())
            })?;
            Ok(Command::Journal { limit })
        }
        "backup" => match arguments.text("what to do with backups")?.as_str() {
            "now" => Ok(Command::BackupNow),
            "list" => Ok(Command::BackupList),
            "restore" => Ok(Command::BackupRestore {
                file: arguments.path("the backup")?,
            }),
            other => Err(ArgsError::Invalid(format!("backup has no {other}"))),
        },
        "rebuild-search" => Ok(Command::RebuildSearch),
        other => Err(ArgsError::Invalid(format!("there is no command {other}"))),
    }
}

fn parse_root(arguments: &mut Arguments) -> Result<Command, ArgsError> {
    match arguments.text("what to do with roots")?.as_str() {
        "add" => {
            let path = arguments.path("the folder")?;
            let mut exclude = Vec::new();
            arguments.options(|option, rest| {
                match option {
                    "--exclude" => exclude.push(rest.text("the pattern")?),
                    other => return Err(unknown(other)),
                }
                Ok(())
            })?;
            Ok(Command::RootAdd { path, exclude })
        }
        "list" => Ok(Command::RootList),
        "remove" => Ok(Command::RootRemove {
            id: arguments.number("the root")?,
        }),
        "relocate" => Ok(Command::RootRelocate {
            id: arguments.number("the root")?,
            path: arguments.path("the folder")?,
        }),
        other => Err(ArgsError::Invalid(format!("root has no {other}"))),
    }
}

fn parse_suggestions(arguments: &mut Arguments) -> Result<Command, ArgsError> {
    let Some(item) = arguments.next_text() else {
        return Ok(Command::Suggestions { all: false });
    };
    match item?.as_str() {
        "--all" => Ok(Command::Suggestions { all: true }),
        "accept" => Ok(Command::SuggestionAccept {
            id: arguments.number("the suggestion")?,
        }),
        "dismiss" => Ok(Command::SuggestionDismiss {
            id: arguments.number("the suggestion")?,
        }),
        other => Err(ArgsError::Invalid(format!("suggestions has no {other}"))),
    }
}

fn parse_edit(arguments: &mut Arguments) -> Result<Command, ArgsError> {
    let path = arguments.path("the file")?;
    let mut edit = Edit::default();
    arguments.options(|option, rest| {
        match option {
            "--title" => edit.title = Some(rest.text("the title")?),
            "--rating" => edit.rating = Some(rest.number("the rating")?),
            "--status" => edit.status = Some(rest.text("the status")?),
            "--series" => edit.series = Some(rest.text("the series")?),
            "--creators" => {
                let creators = rest.text("the creators")?;
                edit.creators = Some(
                    creators
                        .split(';')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect(),
                );
            }
            other => return Err(unknown(other)),
        }
        Ok(())
    })?;
    Ok(Command::Edit { path, edit })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &[&str]) -> Result<Invocation, ArgsError> {
        parse(line.iter().map(OsString::from).collect())
    }

    #[test]
    fn global_options_come_before_the_command() {
        let invocation = parsed(&["--library", "here", "--json", "status"]).unwrap();
        assert_eq!(invocation.library, Some(PathBuf::from("here")));
        assert!(invocation.json);
        assert_eq!(invocation.command, Command::Status);
    }

    #[test]
    fn nothing_at_all_asks_for_help() {
        assert_eq!(parsed(&[]).unwrap_err(), ArgsError::Help);
        assert_eq!(parsed(&["--help"]).unwrap_err(), ArgsError::Help);
    }

    #[test]
    fn commands_with_options() {
        assert_eq!(
            parsed(&["root", "add", "C:/x", "--exclude", "a*", "--exclude", "b"])
                .unwrap()
                .command,
            Command::RootAdd {
                path: PathBuf::from("C:/x"),
                exclude: vec!["a*".to_owned(), "b".to_owned()],
            }
        );
        assert_eq!(
            parsed(&["reconcile", "--root", "3", "--no-hash"])
                .unwrap()
                .command,
            Command::Reconcile {
                root: Some(3),
                hash: false,
            }
        );
        assert_eq!(
            parsed(&["search", "dom casm", "--limit", "5", "--keystrokes"])
                .unwrap()
                .command,
            Command::Search {
                query: "dom casm".to_owned(),
                limit: 5,
                keystrokes: true,
            }
        );
        assert_eq!(
            parsed(&["edit", "a.pdf", "--rating", "4", "--creators", "A; B"])
                .unwrap()
                .command,
            Command::Edit {
                path: PathBuf::from("a.pdf"),
                edit: Edit {
                    rating: Some(4),
                    creators: Some(vec!["A".to_owned(), "B".to_owned()]),
                    ..Edit::default()
                },
            }
        );
    }

    #[test]
    fn targets_can_be_paths_or_ids() {
        assert_eq!(
            parsed(&["inspect", "doc:7"]).unwrap().command,
            Command::Inspect {
                target: Target::Document(7)
            }
        );
        assert_eq!(
            parsed(&["inspect", "work:8"]).unwrap().command,
            Command::Inspect {
                target: Target::Work(8)
            }
        );
        assert_eq!(
            parsed(&["revisions", "C:/a.pdf"]).unwrap().command,
            Command::Revisions {
                target: Target::Path(PathBuf::from("C:/a.pdf"))
            }
        );
    }

    #[test]
    fn mistakes_are_said_as_such() {
        for line in [
            &["nonsense"][..],
            &["root"],
            &["root", "explode"],
            &["root", "remove", "x"],
            &["reconcile", "--wat"],
            &["search"],
            &["inspect", "doc:x"],
            &["move", "a"],
            &["backup", "wat"],
            &["--library"],
            &["undo", "x"],
        ] {
            assert!(
                matches!(parsed(line), Err(ArgsError::Invalid(_))),
                "{line:?}"
            );
        }
    }
}
