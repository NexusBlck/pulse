//! Tasks: a to-do list, kept in `%APPDATA%\Synapse\tasks.json`.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: u64,
    pub text: String,
    pub done: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tasks {
    pub items: Vec<Task>,
    #[serde(default)]
    next_id: u64,
}

fn file() -> std::path::PathBuf {
    crate::config::dir().join("tasks.json")
}

impl Tasks {
    pub fn load() -> Self {
        std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::config::dir());
        if let Ok(t) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(file(), t);
        }
    }

    /// Add a task at the top of the open ones. Blank text adds nothing.
    pub fn add(&mut self, text: &str) -> bool {
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        self.next_id = self.next_id.max(self.items.iter().map(|t| t.id).max().unwrap_or(0)) + 1;
        self.items.insert(0, Task { id: self.next_id, text: text.chars().take(300).collect(), done: false });
        true
    }

    pub fn toggle(&mut self, id: u64) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            t.done = !t.done;
        }
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|t| t.id != id);
    }

    pub fn clear_done(&mut self) {
        self.items.retain(|t| !t.done);
    }

    pub fn left(&self) -> usize {
        self.items.iter().filter(|t| !t.done).count()
    }

    /// Open tasks first (newest on top), then the done ones.
    pub fn ordered(&self) -> Vec<Task> {
        let mut v: Vec<Task> = self.items.iter().filter(|t| !t.done).cloned().collect();
        v.extend(self.items.iter().filter(|t| t.done).cloned());
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list() {
        let mut t = Tasks::default();
        assert!(!t.add("   "));
        t.add("Buy milk");
        t.add("Call mum");
        assert_eq!(t.ordered()[0].text, "Call mum");
        let milk = t.items.iter().find(|x| x.text == "Buy milk").unwrap().id;
        t.toggle(milk);
        assert_eq!(t.left(), 1);
        // Done ones sink to the bottom.
        assert_eq!(t.ordered().last().unwrap().text, "Buy milk");
        t.clear_done();
        assert_eq!(t.items.len(), 1);
        // Ids stay unique after removals.
        let a = t.items[0].id;
        t.remove(a);
        t.add("x");
        assert_ne!(t.items[0].id, a);
    }
}
