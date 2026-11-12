//! Scripted desktop source for measuring and proving monitor screen sharing
//! without a real desktop. Enabled only by `KRASNOKAMEN_SCREEN_TEST_SOURCE`, a
//! comma-separated cycle of `mode:seconds` entries, e.g. `static:12,scroll:12,noise:12,video:12`.
//! It appears in the monitor's window picker as "Test pattern" and never captures anything real.
use super::{DesktopKey, DesktopSource, Frame, PointerButton};
use std::time::{Duration, Instant};

pub(crate) const ID: u64 = 1 << 62;
const WIDTH: usize = 1280;
const HEIGHT: usize = 720;
/// New source frames are produced at most this often (a real capture pushes about this fast).
const INTERVAL: Duration = Duration::from_millis(100);
const TEXT_ROWS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Static,
    Scroll,
    Noise,
    Video,
}

fn script() -> Option<Vec<(Mode, Duration)>> {
    let text = std::env::var("KRASNOKAMEN_SCREEN_TEST_SOURCE").ok()?;
    let mut script = Vec::new();
    for entry in text.split(',') {
        let (name, seconds) = entry.split_once(':')?;
        let mode = match name {
            "static" => Mode::Static,
            "scroll" => Mode::Scroll,
            "noise" => Mode::Noise,
            "video" => Mode::Video,
            _ => return None,
        };
        script.push((mode, Duration::from_secs(seconds.parse().ok()?)));
    }
    (!script.is_empty()).then_some(script)
}

pub(crate) fn enabled() -> bool { script().is_some() }

pub(crate) fn open() -> Result<Box<dyn DesktopSource>, String> {
    let script = script().ok_or("KRASNOKAMEN_SCREEN_TEST_SOURCE is not a valid mode:seconds list")?;
    Ok(Box::new(TestSource { script, started: Instant::now(), last: None, phase: usize::MAX, rng: 0x9E37_79B9_7F4A_7C15, text: text_page() }))
}

struct TestSource {
    script: Vec<(Mode, Duration)>,
    started: Instant,
    last: Option<Instant>,
    phase: usize,
    rng: u64,
    /// A page of pseudo text, `TEXT_ROWS * 2` rows tall, scrolled by wrapping.
    text: Vec<u8>,
}

fn text_page() -> Vec<u8> {
    let rows = TEXT_ROWS * 2;
    let mut page = vec![245u8; WIDTH * rows * 18];
    let mut seed = 12345u32;
    let mut next = move || { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; seed };
    for row in 0..rows {
        let mut x = 24;
        while x < WIDTH - 40 {
            let word = 20 + (next() % 70) as usize;
            for glyph in (x..(x + word).min(WIDTH - 24)).step_by(9) {
                for dy in 3..15 {
                    for dx in 0..6 {
                        if next() % 3 != 0 { page[(row * 18 + dy) * WIDTH + glyph + dx] = 30; }
                    }
                }
            }
            x += word + 12;
        }
    }
    page
}

impl TestSource {
    fn render(&mut self, mode: Mode, elapsed: f64) -> Frame {
        let mut rgba = vec![255u8; WIDTH * HEIGHT * 4];
        match mode {
            Mode::Static | Mode::Scroll => {
                let total = TEXT_ROWS * 2 * 18;
                let offset = if mode == Mode::Scroll { (elapsed * 90.0) as usize % total } else { 0 };
                for y in 0..HEIGHT {
                    let line = &self.text[((y + offset) % total) * WIDTH..][..WIDTH];
                    for (x, value) in line.iter().enumerate() {
                        rgba[(y * WIDTH + x) * 4..][..3].fill(*value);
                    }
                }
                // A fixed window title bar, so the picture is not pure paper.
                for y in 0..24 { for x in 0..WIDTH { rgba[(y * WIDTH + x) * 4..][..3].copy_from_slice(&[40, 60, 120]); } }
            }
            Mode::Noise => {
                for pixel in rgba.chunks_exact_mut(4) {
                    self.rng ^= self.rng << 13; self.rng ^= self.rng >> 7; self.rng ^= self.rng << 17;
                    pixel[..3].copy_from_slice(&self.rng.to_le_bytes()[..3]);
                }
            }
            Mode::Video => {
                let t = elapsed as f32;
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        let (fx, fy) = (x as f32, y as f32);
                        let r = 128.0 + 127.0 * (fx / 90.0 + t * 1.3).sin();
                        let g = 128.0 + 127.0 * (fy / 70.0 - t).sin();
                        let b = 128.0 + 127.0 * ((fx + fy) / 140.0 + t * 0.7).sin();
                        rgba[(y * WIDTH + x) * 4..][..3].copy_from_slice(&[r as u8, g as u8, b as u8]);
                    }
                }
            }
        }
        Frame { width: WIDTH as u32, height: HEIGHT as u32, rgba }
    }
}

impl DesktopSource for TestSource {
    fn next_frame(&mut self) -> Result<Option<Frame>, String> {
        let now = Instant::now();
        if self.last.is_some_and(|last| now.duration_since(last) < INTERVAL) { return Ok(None); }
        self.last = Some(now);
        let cycle: Duration = self.script.iter().map(|(_, length)| *length).sum();
        let mut at = Duration::from_nanos((now.duration_since(self.started).as_nanos() % cycle.as_nanos().max(1)) as u64);
        let mut phase = 0;
        for (index, (_, length)) in self.script.iter().enumerate() {
            phase = index;
            if at < *length { break; }
            at -= *length;
        }
        let mode = self.script[phase].0;
        if phase != self.phase {
            self.phase = phase;
            eprintln!("screen_test_phase mode={mode:?} unix={}", std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs_f64() as u64));
        }
        Ok(Some(self.render(mode, at.as_secs_f64())))
    }
    fn control_granted(&self) -> bool { false }
    fn send_pointer(&mut self, _: f64, _: f64) -> Result<(), String> { Ok(()) }
    fn send_button(&mut self, _: PointerButton, _: bool) -> Result<(), String> { Ok(()) }
    fn send_scroll(&mut self, _: f64, _: f64) -> Result<(), String> { Ok(()) }
    fn send_key(&mut self, _: DesktopKey, _: bool) -> Result<(), String> { Ok(()) }
    fn close(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_repeats_the_same_picture_and_noise_never_does() {
        let mut source = TestSource { script: vec![(Mode::Static, Duration::from_secs(1))], started: Instant::now(), last: None, phase: 0, rng: 1, text: text_page() };
        assert_eq!(source.render(Mode::Static, 0.0).rgba, source.render(Mode::Static, 5.0).rgba);
        assert_ne!(source.render(Mode::Scroll, 0.0).rgba, source.render(Mode::Scroll, 0.5).rgba);
        assert_ne!(source.render(Mode::Noise, 0.0).rgba, source.render(Mode::Noise, 0.0).rgba);
    }
}
