//! Turn a CD-DA track into a level-meter animation.
//!
//! The menu's meter used to be one beat envelope with the bars lagging each
//! other, which keeps time but is not listening: it does the same thing under
//! a breakdown as under a drop. This analyses the audio instead, once, at
//! disc-build time, and ships the result alongside the track.
//!
//! Output is one byte per band per frame, [`FRAME_RATE`] frames a second,
//! bands low to high. The guest looks up a frame by the CD clock and draws it.
//!
//! Bands are normalised per band across the whole track rather than globally.
//! A drum and bass track has far more energy at 60 Hz than at 12 kHz, so a
//! global scale leaves the top half of the meter permanently flat; per band,
//! every bar uses its full height and the meter reads as movement rather than
//! as a bass indicator.

/// Bands across the meter.
pub const BANDS: usize = 16;
/// Frames a second. Two display frames each at 60 Hz.
pub const FRAME_RATE: u32 = 30;

const SAMPLE_RATE: u32 = 44_100;
/// Samples per analysis window. Power of two for the transform, and long
/// enough that the lowest band has something to measure.
const WINDOW: usize = 2048;
/// Lowest and highest frequency the meter covers.
const LOW_HZ: f32 = 40.0;
const HIGH_HZ: f32 = 16_000.0;

/// In-place iterative radix-2 Cooley-Tukey. `re` and `im` are `WINDOW` long.
fn fft(re: &mut [f32; WINDOW], im: &mut [f32; WINDOW]) {
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..WINDOW {
        let mut bit = WINDOW >> 1;
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
    while len <= WINDOW {
        let angle = -2.0 * core::f32::consts::PI / len as f32;
        let (wr, wi) = (angle.cos(), angle.sin());
        let mut start = 0;
        while start < WINDOW {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next_cr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next_cr;
            }
            start += len;
        }
        len <<= 1;
    }
}

/// Which FFT bin each band starts at, log-spaced across the audible range.
fn band_edges() -> [usize; BANDS + 1] {
    let mut edges = [0usize; BANDS + 1];
    let ratio = (HIGH_HZ / LOW_HZ).powf(1.0 / BANDS as f32);
    let bin_hz = SAMPLE_RATE as f32 / WINDOW as f32;
    let mut hz = LOW_HZ;
    let mut previous = 0usize;
    for (i, edge) in edges.iter_mut().enumerate() {
        let mut at = ((hz / bin_hz) as usize).min(WINDOW / 2 - 1);
        // Each band must be at least one bin wide or it measures nothing.
        if i > 0 && at <= previous {
            at = previous + 1;
        }
        *edge = at;
        previous = at;
        hz *= ratio;
    }
    edges
}

/// Analyse `pcm` (raw 44.1 kHz 16-bit stereo, as the disc carries it) into
/// one byte per band per frame.
pub fn analyse(pcm: &[u8]) -> Vec<u8> {
    let (raw, count) = band_energies(pcm);
    normalise(&raw, count)
}

/// Per-frame, per-band loudness in decibels, before any scaling.
fn band_energies(pcm: &[u8]) -> (Vec<f32>, usize) {
    let frames = pcm.len() / 4;
    let hop = (SAMPLE_RATE / FRAME_RATE) as usize;
    let count = frames / hop;
    let edges = band_edges();

    // Hann window, so a drum hit landing at the edge of a window still counts.
    let mut hann = [0.0f32; WINDOW];
    for (i, h) in hann.iter_mut().enumerate() {
        *h = 0.5 - 0.5 * (2.0 * core::f32::consts::PI * i as f32 / WINDOW as f32).cos();
    }

    let mut raw = vec![0.0f32; count * BANDS];
    let mut re = [0.0f32; WINDOW];
    let mut im = [0.0f32; WINDOW];
    for frame in 0..count {
        let start = frame * hop;
        for i in 0..WINDOW {
            let at = start + i;
            let mono = if at < frames {
                let l = i16::from_le_bytes([pcm[at * 4], pcm[at * 4 + 1]]) as f32;
                let r = i16::from_le_bytes([pcm[at * 4 + 2], pcm[at * 4 + 3]]) as f32;
                (l + r) * 0.5
            } else {
                0.0
            };
            re[i] = mono * hann[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        for band in 0..BANDS {
            let mut sum = 0.0f32;
            for bin in edges[band]..edges[band + 1] {
                sum += re[bin] * re[bin] + im[bin] * im[bin];
            }
            let width = (edges[band + 1] - edges[band]).max(1) as f32;
            // Root-mean-square, then decibels: a linear meter spends all its
            // time near zero.
            raw[frame * BANDS + band] = (sum / width).sqrt().max(1.0).log10();
        }
    }
    (raw, count)
}

/// Stretch each band's own middle range across the meter, so every bar uses
/// its full height. The loudest moments clip rather than compressing
/// everything else flat.
///
/// A band that never changes, as in a test tone or silence, has no range to
/// stretch; it pins to one end rather than dividing by zero.
fn normalise(raw: &[f32], count: usize) -> Vec<u8> {
    let mut out = vec![0u8; count * BANDS];
    for band in 0..BANDS {
        let mut column: Vec<f32> = (0..count).map(|f| raw[f * BANDS + band]).collect();
        column.sort_by(|a, b| a.partial_cmp(b).expect("no NaNs from log10 of >= 1"));
        if column.is_empty() {
            continue;
        }
        let floor = column[column.len() / 20]; // 5th percentile
        let ceiling = column[column.len() * 19 / 20]; // 95th
        let range = (ceiling - floor).max(0.001);
        for frame in 0..count {
            let level = ((raw[frame * BANDS + band] - floor) / range * 255.0).clamp(0.0, 255.0);
            out[frame * BANDS + band] = level as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pure tone as raw stereo PCM.
    fn tone(hz: f32, seconds: f32) -> Vec<u8> {
        let frames = (SAMPLE_RATE as f32 * seconds) as usize;
        let mut out = Vec::with_capacity(frames * 4);
        for i in 0..frames {
            let t = i as f32 / SAMPLE_RATE as f32;
            let v = ((2.0 * core::f32::consts::PI * hz * t).sin() * 12000.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn a_frame_is_one_byte_per_band_thirty_times_a_second() {
        let data = analyse(&tone(1000.0, 2.0));
        assert_eq!(data.len() % BANDS, 0);
        let frames = data.len() / BANDS;
        // Two seconds, less the tail that cannot fill a whole hop.
        assert!((58..=60).contains(&frames), "{frames} frames");
    }

    #[test]
    fn a_low_tone_lights_a_lower_band_than_a_high_one() {
        // Against the raw energies: a steady tone has no variation for the
        // per-band scaling to stretch, so the normalised output says nothing
        // about where the tone sat.
        let pick = |hz: f32| {
            let (raw, count) = band_energies(&tone(hz, 3.0));
            let at = (count / 2) * BANDS;
            let frame = &raw[at..at + BANDS];
            frame
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).expect("no NaNs"))
                .map(|(i, _)| i)
                .expect("a frame has bands")
        };
        let low = pick(80.0);
        let high = pick(8000.0);
        assert!(low < high, "80 Hz lit band {low}, 8 kHz lit band {high}");
    }

    #[test]
    fn normalising_stretches_a_bands_own_range_over_the_meter() {
        // One band, rising steadily: the quiet end floors, the loud end pins.
        let count = 100;
        let raw: Vec<f32> = (0..count)
            .flat_map(|f| {
                let mut frame = [0.0f32; BANDS];
                frame[3] = f as f32 / 10.0;
                frame
            })
            .collect();
        let out = normalise(&raw, count);
        assert_eq!(out[3], 0, "quietest frame sits at the bottom");
        assert_eq!(out[(count - 1) * BANDS + 3], 255, "loudest pins the top");
        let middle = out[(count / 2) * BANDS + 3];
        assert!((100..=155).contains(&middle), "middle sits mid-meter: {middle}");
    }

    #[test]
    fn silence_analyses_without_dividing_by_zero() {
        let data = analyse(&vec![0u8; 44_100 * 4]);
        assert_eq!(data.len() / BANDS, 30);
        assert!(data.iter().all(|v| *v == 0 || *v == 255));
    }

    #[test]
    fn bands_are_log_spaced_and_never_empty() {
        let edges = band_edges();
        for i in 0..BANDS {
            assert!(edges[i + 1] > edges[i], "band {i} is empty");
        }
    }
}
