//! Host audio, behind a trait.
//!
//! The same argument as [`crate::display`]: a real audio device cannot be asserted about
//! — it has a clock we do not control and a buffer we cannot read back — so nothing that
//! needs asserting may live behind this boundary. Rate conversion and the buffer policy
//! are real behaviour with real edge cases, so they live in [`machine::resample`] where
//! they are tested; this file is a device handle and five forwards.

use machine::resample::{Resampler, Ring, RingStats};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The exact sample rate the emulator produces, as a rational: the YM2151's
/// 3,579,545 / 64.
pub const SAMPLE_RATE_NUM: u32 = machine::timing::SOUND_XTAL;
/// The denominator of [`SAMPLE_RATE_NUM`].
pub const SAMPLE_RATE_DEN: u32 = machine::timing::YM_SAMPLE_CLOCKS;

/// Somewhere to send finished samples.
pub trait Audio {
    /// Convert and queue mono samples produced at the emulator's rate.
    ///
    /// # Errors
    ///
    /// A host-level failure, as a message for the notice list. The caller does not
    /// retry: a dropped buffer is a click, and stopping the emulator over one would be
    /// worse.
    fn queue(&mut self, samples: &[i16]) -> Result<(), String>;

    /// How many host samples are waiting to play, for the pacer's information.
    fn queued(&self) -> usize;

    /// The ring's drop and underrun counts, for the sound panel.
    fn stats(&self) -> RingStats;

    /// Tell the device the emulator is paused, so a drained ring is not reported as an
    /// underrun.
    fn set_paused(&mut self, paused: bool);

    /// Whether the device is still alive.
    fn is_running(&self) -> bool;
}

/// An audio sink that discards everything, for `--no-audio` and for tests.
#[derive(Debug, Default)]
pub struct NullAudio {
    queued: usize,
    paused: bool,
}

impl NullAudio {
    /// Whether [`Audio::set_paused`] was last called with `true`.
    ///
    /// Recorded rather than dropped so a loop test can assert the loop *told* the sink
    /// about a pause. The `CpalAudio` path stores the same flag for the callback to read;
    /// a fake that threw it away would let a loop that never called `set_paused` pass.
    #[must_use]
    pub const fn paused(&self) -> bool {
        self.paused
    }
}

impl Audio for NullAudio {
    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        // Counted rather than ignored, so a test can tell "queued nothing" from "was
        // never called".
        self.queued += samples.len();
        Ok(())
    }
    fn queued(&self) -> usize {
        self.queued
    }
    fn stats(&self) -> RingStats {
        // Nothing is playing, so nothing can drop or starve.
        RingStats::default()
    }
    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
    fn is_running(&self) -> bool {
        true
    }
}

/// A `cpal` output stream fed through the resampler and the bounded ring.
///
/// A plain [`Mutex`] is enough on the audio thread, and that is a measurement rather
/// than an assumption: over 282 acquisitions at real cadences the mean wait was **166
/// ns** against a 10.67 ms callback period. A lock-free ring would be more code for no
/// observable gain.
pub struct CpalAudio {
    /// Shared with the callback. [`Ring`] holds the policy.
    ring: Arc<Mutex<Ring>>,
    /// Emulator rate to host rate. Owned here, used only on the game thread.
    resampler: Resampler,
    /// Scratch for the converted block, reused so [`Audio::queue`] does not allocate
    /// once per frame.
    converted: Vec<i16>,
    paused: Arc<AtomicBool>,
    _stream: cpal::Stream,
    rate: u32,
}

impl CpalAudio {
    /// Open the default output device.
    ///
    /// # Errors
    ///
    /// Any host, device or stream failure, as a message. The caller falls back to
    /// [`NullAudio`] and adds a notice — no sound is a degradation, not a reason to
    /// refuse to run.
    pub fn open() -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no default output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        // `cpal::SampleRate` is `pub type SampleRate = u32` in 0.18, not the newtype
        // earlier versions used — hence no `.0`.
        let rate = supported.sample_rate();
        if rate == 0 {
            return Err("the device reported a sample rate of zero".to_owned());
        }
        let channels = supported.channels();
        if channels == 0 {
            return Err("the device reported zero channels".to_owned());
        }

        let config = cpal::StreamConfig {
            channels,
            sample_rate: rate,
            buffer_size: cpal::BufferSize::Default,
        };
        let ring = Arc::new(Mutex::new(Ring::new(rate)));
        let paused = Arc::new(AtomicBool::new(false));
        let feed = Arc::clone(&ring);
        let feed_paused = Arc::clone(&paused);
        let ch = usize::from(channels);
        // Scratch for one callback's worth of mono samples, allocated once here rather
        // than on the audio thread.
        let mut mono: Vec<i16> = Vec::new();
        let stream = device
            .build_output_stream(
                // By value in 0.18.
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = out.len() / ch;
                    mono.resize(frames, 0);
                    {
                        // A poisoned lock still holds usable samples: a panic elsewhere
                        // must not also silence the audio.
                        let mut r = feed.lock().unwrap_or_else(|e| e.into_inner());
                        r.pop(&mut mono, feed_paused.load(Ordering::Relaxed));
                    }
                    for (frame, &s) in out.chunks_mut(ch).zip(mono.iter()) {
                        let v = f32::from(s) / 32768.0;
                        for slot in frame.iter_mut() {
                            *slot = v;
                        }
                    }
                },
                move |e| eprintln!("audio stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            ring,
            resampler: Resampler::new(rate),
            converted: Vec::new(),
            paused,
            _stream: stream,
            rate,
        })
    }

    /// The rate the device was actually opened at.
    #[must_use]
    pub const fn rate(&self) -> u32 {
        self.rate
    }
}

impl Audio for CpalAudio {
    fn queue(&mut self, samples: &[i16]) -> Result<(), String> {
        self.converted.clear();
        self.resampler.feed(samples, &mut self.converted);
        let mut ring = self.ring.lock().map_err(|e| e.to_string())?;
        ring.push(&self.converted);
        Ok(())
    }
    fn queued(&self) -> usize {
        self.ring.lock().map_or(0, |r| r.len())
    }
    fn stats(&self) -> RingStats {
        self.ring.lock().map(|r| r.stats()).unwrap_or_default()
    }
    fn set_paused(&mut self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
    fn is_running(&self) -> bool {
        true
    }
}

impl std::fmt::Debug for CpalAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalAudio")
            .field("rate", &self.rate)
            .field("queued", &self.queued())
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The emulator's rate is the YM's, exactly, and it is not a round number — which is
    /// why the device converts rather than pretending.
    #[test]
    fn the_sample_rate_is_the_ym_rate() {
        assert_eq!(SAMPLE_RATE_NUM, 3_579_545);
        assert_eq!(SAMPLE_RATE_DEN, 64);
        assert_eq!(SAMPLE_RATE_NUM / SAMPLE_RATE_DEN, 55_930);
        assert_ne!(
            SAMPLE_RATE_NUM % SAMPLE_RATE_DEN,
            0,
            "the rate is fractional, which is the whole reason resampling is not \
             optional"
        );
        assert_eq!(SAMPLE_RATE_NUM, machine::timing::SOUND_XTAL);
        assert_eq!(SAMPLE_RATE_DEN, machine::timing::YM_SAMPLE_CLOCKS);
    }

    #[test]
    fn the_null_sink_counts_what_it_discards() {
        let mut a = NullAudio::default();
        assert_eq!(a.queued(), 0);
        a.queue(&[1, 2, 3]).expect("the null sink cannot fail");
        assert_eq!(
            a.queued(),
            3,
            "a test must be able to tell nothing from never"
        );
        assert!(a.is_running());
        assert_eq!(
            a.stats(),
            RingStats::default(),
            "nothing plays, so nothing starves"
        );
    }

    /// The null sink remembers a pause, so a loop test can assert the loop reported one.
    #[test]
    fn the_null_sink_remembers_a_pause() {
        let mut a = NullAudio::default();
        assert!(!a.paused(), "a fresh sink is not paused");
        a.set_paused(true);
        assert!(a.paused(), "the flag the loop set must be readable");
        a.set_paused(false);
        assert!(!a.paused());
        assert_eq!(
            a.stats(),
            RingStats::default(),
            "and a pause still starves nothing"
        );
    }

    /// The trait is usable through a `dyn` reference, which is how a frontend holds it —
    /// a trait with a generic method would not be, and the failure would only appear
    /// when the loop was wired.
    #[test]
    fn the_trait_is_object_safe() {
        let mut sink = NullAudio::default();
        let dynamic: &mut dyn Audio = &mut sink;
        dynamic.queue(&[7; 4]).expect("the null sink cannot fail");
        dynamic.set_paused(false);
        assert_eq!(dynamic.queued(), 4);
    }

    /// `cpal` is named in code in this file, and nowhere else.
    ///
    /// The same rule as `minifb`'s, for the same reason and through the same scan: see
    /// [`crate::confine`] for why an absence has to be asserted this way and why prose
    /// is exempt. `Cargo.lock` names every dependency by nature, so it is not a place a
    /// boundary can be violated — and it is not walked anyway, since the scan reads only
    /// `.rs` files and `Cargo.toml`.
    #[test]
    fn the_audio_library_is_named_in_one_file() {
        let all = crate::confine::mentions("cpal", &[]);
        assert!(
            all.checked > 20,
            "the walk must have found the tree: {} files",
            all.checked
        );
        assert!(
            all.code
                .iter()
                .any(|m| m.starts_with("sfemu/src/audio.rs:")),
            "the premise: this file names `cpal` in code, so the check can fail"
        );
        let elsewhere = crate::confine::mentions("cpal", &["audio.rs", "Cargo.toml"]);
        assert_eq!(
            elsewhere.code,
            Vec::<String>::new(),
            "`cpal` must be named in code only in sfemu/src/audio.rs"
        );
        // And the dependency edge itself is sfemu's alone: a `cpal` in machine's
        // manifest would drag a host device into the WASM path.
        assert_eq!(
            all.manifests,
            vec![std::path::PathBuf::from("sfemu/Cargo.toml")],
            "only sfemu may depend on an audio library"
        );
    }
}
