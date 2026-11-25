//! What a notebook keeps beside its sources, each as one file in a folder
//! of its kind: notes, conversations and study sets.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::{Notebook, not_found, unix_secs, valid_id};
use crate::{Error, Result};

/// A file in one of the notebook's folders, by id, with its last change.
pub struct Item {
    pub id: String,
    /// Unix seconds.
    pub updated: u64,
}

/// Conversation the command line writes to, and the one conversation of
/// notebooks made before they could hold several.
pub const FIRST_CHAT: &str = "log";

/// Splits a note file into its title (the first line's `# ` heading) and body.
fn split_note(raw: &str) -> (String, String) {
    match raw.strip_prefix("# ") {
        Some(rest) => {
            let (title, body) = rest.split_once('\n').unwrap_or((rest, ""));
            (
                title.trim().to_string(),
                body.trim_start_matches(['\n', '\r']).to_string(),
            )
        }
        None => (String::new(), raw.to_string()),
    }
}

impl Notebook {
    /// The file of an item in one of the notebook's folders.
    pub fn item_path(&self, folder: &str, id: &str, ext: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err(not_found(format!("Nothing in {folder} is called “{id}”.")));
        }
        Ok(self.dir.join(folder).join(format!("{id}.{ext}")))
    }

    /// Items in a folder with the given extension, most recently changed first.
    pub fn items(&self, folder: &str, ext: &str) -> Result<Vec<Item>> {
        let entries = match std::fs::read_dir(self.dir.join(folder)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut items = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .filter(|id| valid_id(id))
            else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some(ext) {
                continue;
            }
            let updated = entry
                .metadata()
                .and_then(|m| m.modified())
                .map_or(0, unix_secs);
            items.push(Item {
                id: id.to_string(),
                updated,
            });
        }
        items.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id)));
        Ok(items)
    }

    /// An unused id for a new item: the time in milliseconds, so ids sort by age.
    pub fn new_item_id(&self, folder: &str, ext: &str) -> String {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        (ms..)
            .map(|n| n.to_string())
            .find(|id| !self.dir.join(folder).join(format!("{id}.{ext}")).exists())
            .expect("an unused id exists")
    }

    pub fn delete_item(&self, folder: &str, id: &str, ext: &str) -> Result<()> {
        std::fs::remove_file(self.item_path(folder, id, ext)?).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                not_found(format!("Nothing in {folder} is called “{id}”."))
            }
            _ => e.into(),
        })
    }

    pub fn append_chat(&self, chat: &str, record: &impl Serialize) -> Result<()> {
        let line = serde_json::to_string(record).map_err(|e| Error::Invalid(e.to_string()))?;
        let path = self.item_path("chats", chat, "jsonl")?;
        std::fs::create_dir_all(self.dir.join("chats"))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{line}")?;
        Ok(())
    }

    /// A note's title and Markdown body.
    pub fn read_note(&self, id: &str) -> Result<(String, String)> {
        let raw =
            std::fs::read_to_string(self.item_path("notes", id, "md")?).map_err(|e| {
                match e.kind() {
                    std::io::ErrorKind::NotFound => {
                        not_found(format!("No note “{id}” in this notebook."))
                    }
                    _ => e.into(),
                }
            })?;
        Ok(split_note(&raw))
    }

    pub fn write_note(&self, id: &str, title: &str, body: &str) -> Result<()> {
        let path = self.item_path("notes", id, "md")?;
        std::fs::create_dir_all(self.dir.join("notes"))?;
        let title = title.trim().replace(['\n', '\r'], " ");
        let title = if title.is_empty() {
            "Untitled note"
        } else {
            &title
        };
        let tmp = path.with_extension("md.tmp");
        std::fs::write(&tmp, format!("# {title}\n\n{}\n", body.trim_end()))?;
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_keep_title_and_body_and_ids_stay_in_their_folder() {
        let dir = std::env::temp_dir().join(format!("bowerbird-notes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nb = Notebook::open_or_create(&dir, "History").unwrap();
        let id = nb.new_item_id("notes", "md");
        let body = "The war ended in 1648 [[A0|signed in 1648]].\n\n# Not a title";
        nb.write_note(&id, "Causes\nof the war", body).unwrap();
        assert_eq!(
            nb.read_note(&id).unwrap(),
            ("Causes of the war".to_string(), format!("{body}\n"))
        );
        let listed: Vec<String> = nb
            .items("notes", "md")
            .unwrap()
            .into_iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(listed, std::slice::from_ref(&id));
        assert!(nb.item_path("notes", "../notebook", "md").is_err());
        assert!(nb.read_note("missing").is_err());
        nb.delete_item("notes", &id, "md").unwrap();
        assert!(nb.items("notes", "md").unwrap().is_empty());
    }
}
