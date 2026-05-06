#[cfg(windows)]
mod platform {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use rodio::DeviceSinkBuilder;
    use rodio::Source;

    use crate::AUDIO_ENABLED;

    fn is_enabled() -> bool {
        AUDIO_ENABLED.load(Ordering::SeqCst)
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
                let source = rodio::source::SineWave::new(*freq)
                    .take_duration(Duration::from_millis(*ms))
                    .amplify(vol)
                    .fade_in(Duration::from_millis((*ms / 4).max(5)));
                mixer.add(source);
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
                let source = rodio::source::SineWave::new(*freq)
                    .take_duration(Duration::from_millis(ms))
                    .amplify(per_voice)
                    .fade_in(Duration::from_millis((ms / 4).max(5)));
                mixer.add(source);
            }
            std::thread::sleep(Duration::from_millis(ms + 50));
        });
    }

    pub fn play_activate() {
        // Majestic ascending power chord: C5 → E5 → G5 with a final C5+E5+G5 chord
        std::thread::spawn(|| {
            play_notes(&[
                (523.25, 70),   // C5
                (659.25, 70),   // E5
                (783.99, 80),   // G5
            ], 0.06);
            std::thread::sleep(Duration::from_millis(230));
            play_chord(&[523.25, 659.25, 783.99, 1046.50], 200, 0.10); // C major + octave
        });
    }

    pub fn play_deactivate() {
        // Resolving descending: G5 → E5 → C5 minor feel
        play_notes(&[
            (783.99, 70),   // G5
            (622.25, 70),   // Eb5 (minor third)
            (523.25, 120),  // C5
        ], 0.07);
    }

    pub fn play_record_start() {
        // Energetic ascending arpeggio: E4 → G#4 → B4 → E5
        play_notes(&[
            (329.63, 50),   // E4
            (415.30, 50),   // G#4
            (493.88, 50),   // B4
            (659.25, 90),   // E5
        ], 0.06);
    }

    pub fn play_record_stop() {
        // Gentle descending resolution: B4 → G4 → E4
        play_notes(&[
            (493.88, 60),   // B4
            (392.00, 60),   // G4
            (329.63, 100),  // E4
        ], 0.06);
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
