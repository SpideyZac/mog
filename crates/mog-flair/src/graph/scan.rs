//! Finding files and the links between them.

use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use mog_core::walk_files;

/// The most files a graph holds.
const MAX_FILES: usize = 1500;

/// How much of each file is read when looking for links.
const MAX_READ: u64 = 64 * 1024;

/// Extensions of files that make it into the graph.
const EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "ts", "tsx", "mjs", "go", "c", "h", "cpp", "hpp", "cc", "md", "lua",
    "toml", "json",
];

/// Extensions tried when an import leaves the extension off.
const SCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs"];

/// One file in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The file.
    pub path: PathBuf,
    /// The file name.
    pub name: String,
    /// The number of links touching the file.
    pub degree: usize,
}

/// Files and the links between them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Graph {
    /// The files.
    pub nodes: Vec<Node>,
    /// Links as pairs of node indexes, smaller first, without duplicates.
    pub edges: Vec<(usize, usize)>,
}

/// Returns the identifier at the start of `text`.
fn ident(text: &str) -> &str {
    let end = text
        .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
        .unwrap_or(text.len());
    &text[..end]
}

/// Returns every identifier that follows `marker` in `text`.
fn after<'a>(text: &'a str, marker: &str) -> Vec<&'a str> {
    text.match_indices(marker)
        .map(|(at, _)| ident(&text[at + marker.len()..]))
        .filter(|name| !name.is_empty())
        .collect()
}

/// Returns every quoted string that follows `marker`, like the path in `from './x'`.
fn quoted_after<'a>(text: &'a str, marker: &str) -> Vec<&'a str> {
    text.match_indices(marker)
        .filter_map(|(at, _)| {
            let rest = text[at + marker.len()..].trim_start();
            let quote = rest
                .chars()
                .next()
                .filter(|ch| matches!(ch, '\'' | '"' | '`'))?;
            let body = &rest[1..];
            body.find(quote).map(|end| &body[..end])
        })
        .collect()
}

/// Reads the start of a file as text.
fn read_start(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(file) = File::open(path) {
        let _ = file.take(MAX_READ).read_to_string(&mut text);
    }
    text
}

/// Lookups used while resolving links.
struct Index {
    /// Node indexes by path.
    by_path: HashMap<PathBuf, usize>,
    /// Node indexes by lowercase file stem.
    by_stem: HashMap<String, Vec<usize>>,
    /// The `src/lib.rs` or `src/main.rs` node of each Rust crate, by crate identifier.
    crates: HashMap<String, usize>,
}

impl Index {
    /// Returns the node for the first of `candidates` that exists.
    fn first(&self, candidates: &[PathBuf]) -> Option<usize> {
        candidates
            .iter()
            .find_map(|path| self.by_path.get(path).copied())
    }
}

/// Builds the lookups for `nodes`.
fn index(nodes: &[Node]) -> Index {
    let mut by_path = HashMap::new();
    let mut by_stem: HashMap<String, Vec<usize>> = HashMap::new();
    let mut crates = HashMap::new();
    for (i, node) in nodes.iter().enumerate() {
        by_path.insert(node.path.clone(), i);
        if let Some(stem) = node.path.file_stem() {
            by_stem
                .entry(stem.to_string_lossy().to_lowercase())
                .or_default()
                .push(i);
        }
        // crates/<name>/src/lib.rs is the root of crate <name>
        let parts: Vec<String> = node
            .path
            .components()
            .rev()
            .take(3)
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        if let [file, src, name] = parts.as_slice()
            && src == "src"
            && (file == "lib.rs"
                || (file == "main.rs" && !crates.contains_key(&name.replace('-', "_"))))
        {
            crates.insert(name.replace('-', "_"), i);
        }
    }
    Index {
        by_path,
        by_stem,
        crates,
    }
}

/// Returns the `src` folder a Rust file belongs to.
fn rust_src(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|dir| dir.file_name().is_some_and(|name| name == "src"))
}

/// Finds the files a Rust file links to.
fn rust_links(path: &Path, text: &str, index: &Index) -> Vec<usize> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let is_root = matches!(stem.as_str(), "mod" | "lib" | "main");
    let mut links = Vec::new();
    for name in after(text, "mod ") {
        let mut candidates = vec![
            dir.join(format!("{name}.rs")),
            dir.join(name).join("mod.rs"),
        ];
        if !is_root {
            candidates.push(dir.join(&stem).join(format!("{name}.rs")));
        }
        links.extend(index.first(&candidates));
    }
    if let Some(src) = rust_src(path) {
        for name in after(text, "crate::") {
            links.extend(index.first(&[
                src.join(format!("{name}.rs")),
                src.join(name).join("mod.rs"),
            ]));
        }
    }
    for name in after(text, "super::") {
        let parent = if is_root {
            dir.parent().unwrap_or(dir)
        } else {
            dir
        };
        links.extend(index.first(&[
            parent.join(format!("{name}.rs")),
            parent.join(name).join("mod.rs"),
        ]));
    }
    for (crate_name, &root) in &index.crates {
        if text.contains(&format!("{crate_name}::")) || text.contains(&format!("use {crate_name}"))
        {
            links.push(root);
        }
    }
    links
}

/// Finds the files a script imports with relative paths.
fn script_links(path: &Path, text: &str, index: &Index) -> Vec<usize> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let mut specs = quoted_after(text, "from");
    specs.extend(quoted_after(text, "require("));
    specs.extend(quoted_after(text, "import("));
    specs.extend(quoted_after(text, "import"));
    specs
        .into_iter()
        .filter(|spec| spec.starts_with('.'))
        .filter_map(|spec| {
            let base = normalize(&dir.join(spec));
            let mut candidates = vec![base.clone()];
            for ext in SCRIPT_EXTENSIONS {
                candidates.push(base.with_extension(ext));
                candidates.push(base.join(format!("index.{ext}")));
            }
            index.first(&candidates)
        })
        .collect()
}

/// Finds the modules a Python file imports.
fn python_links(path: &Path, text: &str, root: &Path, index: &Index) -> Vec<usize> {
    let dir = path.parent().unwrap_or(Path::new(""));
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start();
            let module = line
                .strip_prefix("from ")
                .or_else(|| line.strip_prefix("import "))?
                .split_whitespace()
                .next()?;
            let dots = module.chars().take_while(|ch| *ch == '.').count();
            let rel = module[dots..].replace('.', "/");
            let base = if dots > 0 {
                dir.to_path_buf()
            } else {
                root.to_path_buf()
            };
            let file = base.join(format!("{rel}.py"));
            let package = base.join(&rel).join("__init__.py");
            index.first(&[file, package, dir.join(format!("{rel}.py"))])
        })
        .collect()
}

/// Finds the headers a C file includes with quotes.
fn c_links(path: &Path, text: &str, index: &Index) -> Vec<usize> {
    let dir = path.parent().unwrap_or(Path::new(""));
    quoted_after(text, "#include")
        .into_iter()
        .filter_map(|header| index.first(&[normalize(&dir.join(header))]))
        .collect()
}

/// Finds the notes a Markdown file links to.
fn markdown_links(path: &Path, text: &str, index: &Index) -> Vec<usize> {
    let dir = path.parent().unwrap_or(Path::new(""));
    let mut links = Vec::new();
    for (at, _) in text.match_indices("[[") {
        let rest = &text[at + 2..];
        if let Some(end) = rest.find("]]") {
            let name = rest[..end]
                .split('|')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if let Some(found) = index.by_stem.get(&name) {
                links.extend(found.first());
            }
        }
    }
    for (at, _) in text.match_indices("](") {
        let rest = &text[at + 2..];
        if let Some(end) = rest.find(')') {
            let target = rest[..end].split('#').next().unwrap_or("");
            if !target.contains("://") && !target.is_empty() {
                links.extend(index.first(&[normalize(&dir.join(target))]));
            }
        }
    }
    links
}

/// Resolves `.` and `..` in `path` without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part.as_os_str().to_str() {
            Some(".") => {}
            Some("..") => {
                out.pop();
            }
            _ => out.push(part),
        }
    }
    out
}

/// Scans the project at `root` and returns its files and links.
pub fn scan(root: &Path) -> Graph {
    let mut nodes: Vec<Node> = walk_files(root, MAX_FILES * 4)
        .into_iter()
        .filter(|path| {
            path.extension().is_some_and(|ext| {
                EXTENSIONS.contains(&ext.to_string_lossy().to_lowercase().as_str())
            })
        })
        .take(MAX_FILES)
        .map(|path| Node {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: normalize(&path),
            degree: 0,
        })
        .collect();
    let index = index(&nodes);
    let mut edges = HashSet::new();
    for (from, node) in nodes.iter().enumerate() {
        let ext = node
            .path
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let text = read_start(&node.path);
        let links = match ext.as_str() {
            "rs" => rust_links(&node.path, &text, &index),
            "js" | "jsx" | "ts" | "tsx" | "mjs" => script_links(&node.path, &text, &index),
            "py" => python_links(&node.path, &text, root, &index),
            "c" | "h" | "cpp" | "hpp" | "cc" => c_links(&node.path, &text, &index),
            "md" => markdown_links(&node.path, &text, &index),
            _ => Vec::new(),
        };
        for to in links {
            if to != from {
                edges.insert((from.min(to), from.max(to)));
            }
        }
    }
    let mut edges: Vec<(usize, usize)> = edges.into_iter().collect();
    edges.sort_unstable();
    for &(a, b) in &edges {
        nodes[a].degree += 1;
        nodes[b].degree += 1;
    }
    Graph { nodes, edges }
}

#[cfg(test)]
/// Tests for link scanning.
mod tests {
    use std::{env, fs, path::PathBuf, process};

    use super::{after, quoted_after, scan};

    /// Creates a scratch project for `name`.
    fn project(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("mog-graph-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    /// Identifiers and quoted paths are pulled out after markers.
    #[test]
    fn extracts_names() {
        assert_eq!(after("mod a; pub mod b_c;", "mod "), ["a", "b_c"]);
        assert_eq!(quoted_after("import x from './x';", "from"), ["./x"]);
    }

    /// Rust modules, script imports and markdown links all become edges.
    #[test]
    fn links_files() {
        let dir = project("links");
        let src = dir.join("src");
        fs::create_dir_all(src.join("web")).expect("mkdir");
        fs::write(src.join("main.rs"), "mod util;\nuse crate::util::x;").expect("write");
        fs::write(src.join("util.rs"), "pub fn x() {}").expect("write");
        fs::write(
            src.join("web").join("app.ts"),
            "import { h } from './helper';",
        )
        .expect("write");
        fs::write(src.join("web").join("helper.ts"), "export const h = 1;").expect("write");
        fs::write(
            dir.join("notes.md"),
            "see [[README]] and [main](src/main.rs)",
        )
        .expect("write");
        fs::write(dir.join("README.md"), "hi").expect("write");
        let graph = scan(&dir);
        let _ = fs::remove_dir_all(&dir);
        let linked = |a: &str, b: &str| {
            let find = |name| graph.nodes.iter().position(|node| node.name == name);
            let (Some(a), Some(b)) = (find(a), find(b)) else {
                return false;
            };
            graph.edges.contains(&(a.min(b), a.max(b)))
        };
        assert!(linked("main.rs", "util.rs"));
        assert!(linked("app.ts", "helper.ts"));
        assert!(linked("notes.md", "README.md"));
        assert!(linked("notes.md", "main.rs"));
        assert_eq!(graph.edges.len(), 4);
    }
}
