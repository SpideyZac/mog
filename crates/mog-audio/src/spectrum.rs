//! Turning raw samples into spectrum bars.

use std::f32::consts::PI;

/// The lowest frequency the bars show, in hertz.
const LOW_HZ: f32 = 40.0;

/// The highest frequency the bars show, in hertz.
const HIGH_HZ: f32 = 16_000.0;

/// The level in decibels that counts as an empty bar.
const FLOOR_DB: f32 = -70.0;

/// The level in decibels that counts as a full bar.
const CEILING_DB: f32 = -10.0;

/// Transforms `re` and `im` in place with an iterative radix 2 FFT.
///
/// # Panics
///
/// Panics if the lengths differ or are not a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    assert!(
        n.is_power_of_two() && im.len() == n,
        "fft needs a power of two"
    );
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = -2.0 * PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (sin, cos) = (angle * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cos - im[b] * sin;
                let ti = re[b] * sin + im[b] * cos;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Returns `bands` levels from 0 to 1 for `samples` recorded at `sample_rate`.
///
/// Bands are spaced evenly in pitch, so bass gets as much room as treble like on a stereo.
/// `samples` should be a power of two long, extra samples at the front are ignored.
pub fn bands(samples: &[f32], sample_rate: u32, bands: usize) -> Vec<f32> {
    let n = if samples.len().is_power_of_two() {
        samples.len()
    } else {
        samples.len().next_power_of_two() / 2
    };
    if n < 2 || bands == 0 || sample_rate == 0 {
        return vec![0.0; bands];
    }
    let samples = &samples[samples.len() - n..];
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            // a hann window keeps loud bins from smearing into their neighbours
            let window = 0.5 - 0.5 * (2.0 * PI * i as f32 / (n - 1) as f32).cos();
            sample * window
        })
        .collect();
    let mut im = vec![0.0; n];
    fft(&mut re, &mut im);
    let bin_hz = sample_rate as f32 / n as f32;
    let high = HIGH_HZ.min(sample_rate as f32 / 2.0);
    let ratio = (high / LOW_HZ).powf(1.0 / bands as f32);
    (0..bands)
        .map(|band| {
            let from = LOW_HZ * ratio.powi(band as i32);
            let to = from * ratio;
            // the clamps keep the casts in range
            let first = ((from / bin_hz).floor().max(1.0) as usize).min(n / 2 - 1);
            let last = ((to / bin_hz).ceil() as usize).clamp(first + 1, n / 2);
            let peak = (first..last)
                .map(|bin| (re[bin] * re[bin] + im[bin] * im[bin]).sqrt())
                .fold(0.0, f32::max);
            // a full scale sine peaks at a quarter of the window length after windowing
            let db = 20.0 * (peak / (n as f32 / 4.0)).max(1e-9).log10();
            ((db - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
/// Tests for the spectrum.
mod tests {
    use std::f32::consts::PI;

    use super::bands;

    /// A pure tone lights up one band and leaves the far ones empty.
    #[test]
    fn tone_hits_its_band() {
        let rate = 48_000;
        let samples: Vec<f32> = (0..2048)
            .map(|i| (2.0 * PI * 1000.0 * i as f32 / rate as f32).sin() * 0.5)
            .collect();
        let levels = bands(&samples, rate, 16);
        let loudest = levels
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, _)| i)
            .expect("bands");
        // 1 khz sits a little past the middle of 40 hz to 16 khz in pitch
        assert!((7..=9).contains(&loudest), "{levels:?}");
        assert!(levels[0] < 0.2 && levels[15] < 0.2, "{levels:?}");
    }

    /// Silence gives empty bars.
    #[test]
    fn silence_is_empty() {
        assert!(bands(&[0.0; 1024], 44_100, 8).iter().all(|&l| l == 0.0));
    }
}
