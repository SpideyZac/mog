//! A folder shown as an expandable tree of files.

use std::{
    collections::HashSet,
    fs, io,
    path::{self, Path, PathBuf},
};

/// Entry names that are never listed.
const HIDDEN: [&str; 1] = [".git"];

/// One visible row of a [`FileTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The full path of the file or folder.
    pub path: PathBuf,
    /// The file or folder name.
    pub name: String,
    /// How deeply the entry is nested below the root, starting at 0.
    pub depth: usize,
    /// Whether the entry is a folder.
    pub is_dir: bool,
    /// Whether the entry is a folder that is expanded.
    pub expanded: bool,
}

/// A folder and the subfolders the user has expanded, flattened into rows.
#[derive(Debug, Clone)]
pub struct FileTree {
    /// The folder at the top of the tree.
    root: PathBuf,
    /// The folders that are expanded.
    expanded: HashSet<PathBuf>,
    /// The visible rows, top to bottom.
    entries: Vec<Entry>,
}

impl FileTree {
    /// Creates a tree of the folder at `root` with everything collapsed.
    ///
    /// # Errors
    ///
    /// Returns an error if `root` cannot be read as a folder.
    pub fn new(root: impl Into<PathBuf>) -> io::Result<Self> {
        let root = root.into();
        // language servers want absolute paths so resolve it once here
        let root = path::absolute(&root).unwrap_or(root);
        fs::read_dir(&root)?;
        let mut tree = Self {
            root,
            expanded: HashSet::new(),
            entries: Vec::new(),
        };
        tree.refresh();
        Ok(tree)
    }

    /// Returns the folder at the top of the tree.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the name of the root folder.
    pub fn name(&self) -> String {
        self.root.file_name().map_or_else(
            || self.root.display().to_string(),
            |name| name.to_string_lossy().into(),
        )
    }

    /// Returns the visible rows.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Expands or collapses the folder at `index`. Does nothing for files.
    pub fn toggle(&mut self, index: usize) {
        let Some(entry) = self.entries.get(index).filter(|entry| entry.is_dir) else {
            return;
        };
        if !self.expanded.remove(&entry.path) {
            self.expanded.insert(entry.path.clone());
        }
        self.refresh();
    }

    /// Reads the folders again to pick up files created or removed since.
    pub fn refresh(&mut self) {
        let mut entries = Vec::new();
        self.list(&self.root, 0, &mut entries);
        self.entries = entries;
    }

    /// Appends the rows for the contents of `dir` and its expanded subfolders.
    ///
    /// Folders that cannot be read are shown empty.
    fn list(&self, dir: &Path, depth: usize, out: &mut Vec<Entry>) {
        let Ok(read) = fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<Entry> = read
            .flatten()
            .filter_map(|child| {
                let name = child.file_name().to_string_lossy().into_owned();
                if HIDDEN.contains(&name.as_str()) {
                    return None;
                }
                let path = child.path();
                // follow symlinks so linked folders still expand
                let is_dir = path.is_dir();
                Some(Entry {
                    expanded: is_dir && self.expanded.contains(&path),
                    path,
                    name,
                    depth,
                    is_dir,
                })
            })
            .collect();
        children.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        for child in children {
            let expanded = child.expanded.then(|| child.path.clone());
            out.push(child);
            if let Some(path) = expanded {
                self.list(&path, depth + 1, out);
            }
        }
    }
}

#[cfg(test)]
/// Tests for [`FileTree`].
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use super::FileTree;

    /// Creates an empty scratch folder unique to `name`.
    fn scratch(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mog-tree-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch dir");
        dir
    }

    /// Returns the indented names of the visible rows.
    fn names(tree: &FileTree) -> Vec<String> {
        tree.entries()
            .iter()
            .map(|entry| format!("{}{}", " ".repeat(entry.depth), entry.name))
            .collect()
    }

    /// Folders come first, names sort without case and `.git` is hidden.
    #[test]
    fn lists_folders_first() {
        let dir = scratch("sort");
        fs::create_dir(dir.join("src")).expect("mkdir");
        fs::create_dir(dir.join(".git")).expect("mkdir");
        fs::write(dir.join("b.txt"), "").expect("write");
        fs::write(dir.join("A.txt"), "").expect("write");
        let tree = FileTree::new(&dir).expect("tree");
        assert_eq!(names(&tree), ["src", "A.txt", "b.txt"]);
        let _ = fs::remove_dir_all(dir);
    }

    /// Toggling a folder shows and hides its contents.
    #[test]
    fn toggle_expands_and_collapses() {
        let dir = scratch("toggle");
        fs::create_dir(dir.join("src")).expect("mkdir");
        fs::write(dir.join("src").join("main.rs"), "").expect("write");
        fs::write(dir.join("README.md"), "").expect("write");
        let mut tree = FileTree::new(&dir).expect("tree");
        tree.toggle(0);
        assert_eq!(names(&tree), ["src", " main.rs", "README.md"]);
        assert!(tree.entries()[0].expanded);
        tree.toggle(0);
        assert_eq!(names(&tree), ["src", "README.md"]);
        let _ = fs::remove_dir_all(dir);
    }

    /// A file is not a folder that can be opened as a tree.
    #[test]
    fn file_is_not_a_tree() {
        let dir = scratch("file");
        let file = dir.join("x.txt");
        fs::write(&file, "").expect("write");
        assert!(FileTree::new(&file).is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
