//! Sound effects, synthesised at start-up.
//!
//! A few lines of arithmetic each: no audio files to license or lose, and the
//! whole set costs nothing in the executable's size.

use macroquad::audio::{load_sound_from_bytes, play_sound, PlaySoundParams, Sound};

const RATE: u32 = 44_100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sfx {
    Explosion,
    BombPlaced,
    Pickup,
    Death,
    Beep,
    Go,
    Win,
    Click,
}

pub struct Sounds {
    table: Vec<(Sfx, Sound)>,
    pub enabled: bool,
}

impl Sounds {
    pub async fn load(enabled: bool) -> Sounds {
        let mut table = Vec::new();
        for (sfx, samples) in [
            (Sfx::Explosion, explosion()),
            (Sfx::BombPlaced, thump()),
            (Sfx::Pickup, arpeggio(&[660.0, 880.0, 1320.0], 0.06, 0.35)),
            (Sfx::Death, sweep(520.0, 90.0, 0.55, 0.4)),
            (Sfx::Beep, tone(660.0, 0.12, 0.35)),
            (Sfx::Go, tone(990.0, 0.30, 0.4)),
            (
                Sfx::Win,
                arpeggio(&[523.3, 659.3, 784.0, 1046.5], 0.12, 0.35),
            ),
            (Sfx::Click, tone(1200.0, 0.03, 0.2)),
        ] {
            // A machine without an audio device still gets a game.
            if let Ok(sound) = load_sound_from_bytes(&wav(&samples)).await {
                table.push((sfx, sound));
            }
        }
        Sounds { table, enabled }
    }

    pub fn play(&self, sfx: Sfx) {
        self.play_at(sfx, 1.0);
    }

    pub fn play_at(&self, sfx: Sfx, volume: f32) {
        if !self.enabled {
            return;
        }
        if let Some((_, sound)) = self.table.iter().find(|(s, _)| *s == sfx) {
            play_sound(
                sound,
                PlaySoundParams {
                    looped: false,
                    volume,
                },
            );
        }
    }
}

/// Deterministic noise, so every run sounds the same.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn len(seconds: f32) -> usize {
    (seconds * RATE as f32) as usize
}

/// Low-passed noise with a fast attack and a long tail, plus a sub-bass drop.
fn explosion() -> Vec<f32> {
    let n = len(0.9);
    let mut noise = Noise(0x9E37_79B9);
    let mut lp = 0.0f32;
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let env = (1.0 - t / 0.9).max(0.0).powf(2.2) * (t / 0.005).min(1.0);
            // Cutoff falls as the blast rolls off.
            let alpha = 0.25 * (1.0 - t / 0.9).max(0.05);
            lp += alpha * (noise.next() - lp);
            let freq = 90.0 * (1.0 - t).max(0.3);
            phase += freq / RATE as f32;
            let sub = (phase * std::f32::consts::TAU).sin() * (1.0 - t / 0.4).max(0.0);
            ((lp * 1.8 + sub * 0.6) * env * 0.8).clamp(-1.0, 1.0)
        })
        .collect()
}

fn thump() -> Vec<f32> {
    sweep(220.0, 70.0, 0.12, 0.6)
}

fn tone(freq: f32, seconds: f32, volume: f32) -> Vec<f32> {
    let n = len(seconds);
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let env = (1.0 - t / seconds).max(0.0) * (t / 0.004).min(1.0);
            // A touch of the octave makes a plain sine sound less like a test signal.
            let s = (t * freq * std::f32::consts::TAU).sin()
                + 0.3 * (t * freq * 2.0 * std::f32::consts::TAU).sin();
            s * env * volume
        })
        .collect()
}

fn sweep(from: f32, to: f32, seconds: f32, volume: f32) -> Vec<f32> {
    let n = len(seconds);
    let mut phase = 0.0f32;
    (0..n)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let k = t / seconds;
            let freq = from + (to - from) * k;
            phase += freq / RATE as f32;
            let env = (1.0 - k).max(0.0) * (t / 0.003).min(1.0);
            // Square-ish: a softly clipped sine, for an 8-bit colour.
            ((phase * std::f32::consts::TAU).sin() * 2.5).clamp(-1.0, 1.0) * env * volume
        })
        .collect()
}

fn arpeggio(notes: &[f32], each: f32, volume: f32) -> Vec<f32> {
    notes
        .iter()
        .flat_map(|&f| tone(f, each * 1.6, volume).into_iter().take(len(each)))
        .chain(tone(*notes.last().unwrap(), each * 2.0, volume))
        .collect()
}

/// 16-bit mono PCM in a RIFF container.
fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wav_header_describes_its_data() {
        let bytes = wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
    }

    #[test]
    fn every_effect_is_audible_and_in_range() {
        for samples in [
            explosion(),
            thump(),
            tone(440.0, 0.1, 0.3),
            sweep(500.0, 90.0, 0.5, 0.4),
        ] {
            assert!(!samples.is_empty());
            assert!(samples.iter().all(|s| s.abs() <= 1.0));
            assert!(samples.iter().any(|s| s.abs() > 0.05));
        }
    }
}
