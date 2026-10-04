//! Tiny text charts for flair.

/// The bar heights a sparkline is drawn with, lowest first.
const BARS: [char; 8] = [
    '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}',
];

/// Draws `values` as a sparkline, with `lo` as the lowest bar and `hi` as the highest.
pub fn sparkline(values: &[f32], lo: f32, hi: f32) -> String {
    let span = (hi - lo).max(f32::EPSILON);
    values
        .iter()
        .map(|value| {
            let level = ((value - lo) / span * 7.0).round().clamp(0.0, 7.0);
            // the clamp keeps the index inside the bars
            BARS[level as usize]
        })
        .collect()
}

/// Draws a bar `width` cells long filled to `fraction`.
pub fn bar(fraction: f32, width: usize) -> String {
    let width_f = f32::from(u16::try_from(width).unwrap_or(u16::MAX));
    // the clamp keeps the cast inside the width
    let filled = (fraction.clamp(0.0, 1.0) * width_f).round() as usize;
    format!(
        "{}{}",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(width - filled.min(width))
    )
}

#[cfg(test)]
/// Tests for the text charts.
mod tests {
    use super::{bar, sparkline};

    /// Sparklines go from the lowest to the highest bar.
    #[test]
    fn sparkline_spans_bars() {
        assert_eq!(
            sparkline(&[0.0, 50.0, 100.0], 0.0, 100.0),
            "\u{2581}\u{2585}\u{2588}"
        );
        assert_eq!(sparkline(&[200.0], 0.0, 100.0), "\u{2588}");
    }

    /// Bars fill in proportion and keep their width.
    #[test]
    fn bar_fills() {
        assert_eq!(bar(0.5, 4), "\u{2588}\u{2588}\u{2591}\u{2591}");
        assert_eq!(bar(2.0, 3).chars().count(), 3);
        assert_eq!(bar(-1.0, 3), "\u{2591}\u{2591}\u{2591}");
    }
}
