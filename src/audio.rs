#[cfg(windows)]
mod platform {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use rodio::DeviceSinkBuilder;
    use rodio::Source;

    use crate::AUDIO_ENABLED;

    const E4: f32 = 329.63;
    const G4: f32 = 392.00;
    const GS4: f32 = 415.30;
    const B4: f32 = 493.88;
    const C5: f32 = 523.25;
    const EB5: f32 = 622.25;
    const E5: f32 = 659.25;
    const G5: f32 = 783.99;
    const C6: f32 = 1046.50;

    fn is_enabled() -> bool {
        AUDIO_ENABLED.load(Ordering::SeqCst)
    }

    /// A sine tone with the shared duration/volume/fade-in shaping.
    fn sine(freq: f32, ms: u64, vol: f32) -> impl Source<Item = f32> {
        rodio::source::SineWave::new(freq)
            .take_duration(Duration::from_millis(ms))
            .amplify(vol)
            .fade_in(Duration::from_millis((ms / 4).max(5)))
    }

    /// Play a sequence of notes. Each note is (frequency_hz, duration_ms).
    fn play_notes(notes: &[(f32, u64)], vol: f32) {
        if !is_enabled() {
            return;
        }
        let notes: Vec<(f32, u64)> = notes.to_vec();
        std::thread::spawn(move || {
            let Ok(mut handle) = DeviceSinkBuilder::open_default_sink() else {
                return;
            };
            handle.log_on_drop(false);
            let mixer = handle.mixer();
            for (freq, ms) in &notes {
                mixer.add(sine(*freq, *ms, vol));
                std::thread::sleep(Duration::from_millis(*ms));
            }
            std::thread::sleep(Duration::from_millis(50));
        });
    }

    /// Play a chord (multiple notes at once).
    fn play_chord(freqs: &[f32], ms: u64, vol: f32) {
        if !is_enabled() {
            return;
        }
        let freqs: Vec<f32> = freqs.to_vec();
        std::thread::spawn(move || {
            let Ok(mut handle) = DeviceSinkBuilder::open_default_sink() else {
                return;
            };
            handle.log_on_drop(false);
            let mixer = handle.mixer();
            let per_voice = vol / freqs.len().max(1) as f32;
            for freq in &freqs {
                mixer.add(sine(*freq, ms, per_voice));
            }
            std::thread::sleep(Duration::from_millis(ms + 50));
        });
    }

    pub fn play_activate() {
        std::thread::spawn(|| {
            play_notes(&[(C5, 70), (E5, 70), (G5, 80)], 0.06);
            std::thread::sleep(Duration::from_millis(230));
            play_chord(&[C5, E5, G5, C6], 200, 0.10);
        });
    }

    pub fn play_deactivate() {
        play_notes(&[(G5, 70), (EB5, 70), (C5, 120)], 0.07);
    }

    pub fn play_record_start() {
        play_notes(&[(E4, 50), (GS4, 50), (B4, 50), (E5, 90)], 0.06);
    }

    pub fn play_record_stop() {
        play_notes(&[(B4, 60), (G4, 60), (E4, 100)], 0.06);
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn play_activate() {}
    pub fn play_deactivate() {}
    pub fn play_record_start() {}
    pub fn play_record_stop() {}
}

pub use platform::*;
