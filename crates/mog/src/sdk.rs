//! The plugin SDKs that ship inside mog, written to disk so plugins can import them without
//! carrying a copy that drifts from the editor they run in.

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use mog_config::state_dir;

/// The Python SDK.
pub const PYTHON: &str = include_str!("../../../sdk/python/mog_plugin.py");

/// The JavaScript SDK.
pub const NODE: &str = include_str!("../../../sdk/node/mog-plugin.js");

/// The folder the SDKs were written to, once they are.
static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Writes `contents` to `path` unless it already holds exactly that.
fn write_if_changed(path: &Path, contents: &str) -> Option<()> {
    if fs::read_to_string(path).ok().as_deref() == Some(contents) {
        return Some(());
    }
    fs::create_dir_all(path.parent()?).ok()?;
    fs::write(path, contents).ok()
}

/// Returns the folder holding `python/mog_plugin.py` and `node/mog-plugin.js` for this
/// version of mog, writing them the first time it is asked.
///
/// Returns `None` if they could not be written, plugins then need their own copy.
pub fn dir() -> Option<PathBuf> {
    DIR.get_or_init(|| {
        let base = state_dir().unwrap_or_else(env::temp_dir);
        let dir = base.join("sdk").join(env!("CARGO_PKG_VERSION"));
        write_if_changed(&dir.join("python").join("mog_plugin.py"), PYTHON)?;
        write_if_changed(&dir.join("node").join("mog-plugin.js"), NODE)?;
        Some(dir)
    })
    .clone()
}

#[cfg(test)]
/// Tests for the bundled SDKs.
mod tests {
    use std::fs;

    use super::{NODE, PYTHON, dir};

    /// Both SDKs are written where plugins look for them.
    #[test]
    fn writes_the_sdks() {
        let dir = dir().expect("written");
        let python = fs::read_to_string(dir.join("python/mog_plugin.py")).expect("python");
        let node = fs::read_to_string(dir.join("node/mog-plugin.js")).expect("node");
        assert_eq!(python, PYTHON);
        assert_eq!(node, NODE);
    }
}
