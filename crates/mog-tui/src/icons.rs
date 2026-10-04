//! Little colored markers for file types.

use ratatui::style::Color;

/// Marker colors by file extension.
const BY_EXTENSION: &[(&str, Color)] = &[
    ("rs", Color::Rgb(255, 140, 70)),
    ("toml", Color::Rgb(160, 160, 170)),
    ("md", Color::Rgb(100, 180, 255)),
    ("py", Color::Rgb(255, 214, 80)),
    ("js", Color::Rgb(240, 220, 80)),
    ("jsx", Color::Rgb(97, 218, 251)),
    ("ts", Color::Rgb(49, 120, 198)),
    ("tsx", Color::Rgb(97, 218, 251)),
    ("json", Color::Rgb(250, 200, 90)),
    ("go", Color::Rgb(0, 173, 216)),
    ("c", Color::Rgb(85, 140, 220)),
    ("h", Color::Rgb(150, 120, 220)),
    ("cpp", Color::Rgb(240, 80, 120)),
    ("lua", Color::Rgb(80, 80, 230)),
    ("sh", Color::Rgb(120, 220, 120)),
    ("html", Color::Rgb(230, 90, 50)),
    ("css", Color::Rgb(90, 140, 240)),
    ("lock", Color::Rgb(110, 100, 130)),
    ("yml", Color::Rgb(200, 80, 80)),
    ("yaml", Color::Rgb(200, 80, 80)),
    ("hpp", Color::Rgb(240, 80, 120)),
    ("cc", Color::Rgb(240, 80, 120)),
    ("cs", Color::Rgb(104, 33, 122)),
    ("java", Color::Rgb(230, 120, 30)),
    ("kt", Color::Rgb(170, 110, 250)),
    ("rb", Color::Rgb(205, 40, 40)),
    ("php", Color::Rgb(120, 125, 180)),
    ("swift", Color::Rgb(250, 110, 60)),
    ("scala", Color::Rgb(220, 50, 50)),
    ("hs", Color::Rgb(150, 110, 200)),
    ("ml", Color::Rgb(240, 140, 30)),
    ("ex", Color::Rgb(110, 75, 160)),
    ("exs", Color::Rgb(110, 75, 160)),
    ("zig", Color::Rgb(245, 165, 35)),
    ("sql", Color::Rgb(220, 180, 120)),
    ("ps1", Color::Rgb(80, 150, 230)),
    ("xml", Color::Rgb(230, 140, 60)),
    ("svg", Color::Rgb(255, 180, 50)),
    ("dart", Color::Rgb(60, 180, 230)),
    ("vue", Color::Rgb(65, 184, 131)),
];

/// The marker drawn before file names.
pub const FILE_MARKER: &str = "\u{25cf}";

/// Returns the marker color for a file called `name`, or `None` for unknown types.
pub fn file_color(name: &str) -> Option<Color> {
    let extension = name.rsplit_once('.')?.1.to_lowercase();
    BY_EXTENSION
        .iter()
        .find(|(ext, _)| *ext == extension)
        .map(|(_, color)| *color)
}

#[cfg(test)]
/// Tests for file markers.
mod tests {
    use super::file_color;

    /// Known extensions get a color and unknown ones do not.
    #[test]
    fn colors_by_extension() {
        assert!(file_color("main.RS").is_some());
        assert!(file_color("Makefile").is_none());
        assert!(file_color("x.unknown").is_none());
    }
}
