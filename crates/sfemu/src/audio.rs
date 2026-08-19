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
    /// Convert and queue interleaved stereo samples produced at the emulator's rate.
    ///
    /// Two channels, L,R, always — [`machine::resample::CHANNELS`]. A mono board
    /// writes its one value into both slots rather than there being a mono path.
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

    /// Whether anything is actually playing.
    ///
    /// `false` for [`NullAudio`], which is what the title bar reads to say `[no audio]`:
    /// a player who hears nothing needs to be told whether the emulator is silent
    /// because the device could not be opened or because the game is.
    fn is_running(&self) -> bool;
}

/// An audio sink that discards everything, for a host with no usable device.
#[derive(Debug, Default)]
pub struct NullAudio {
    queued: usize,
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
    fn set_paused(&mut self, _paused: bool) {
        // Nothing to tell: the flag exists so a real device's callback stops counting
        // underruns against a deliberately empty ring, and this sink has no ring. What
        // matters — that the *loop* reports the pause — is asserted in `loop_`'s tests
        // against a fake that records every call.
    }
    fn is_running(&self) -> bool {
        false
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
        // Scratch for one callback's worth of interleaved stereo, allocated once here
        // rather than on the audio thread.
        let mut stereo: Vec<i16> = Vec::new();
        let stream = device
            .build_output_stream(
                // By value in 0.18.
                config,
                move |out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    let frames = out.len() / ch;
                    stereo.resize(frames * machine::resample::CHANNELS, 0);
                    {
                        // A poisoned lock still holds usable samples: a panic elsewhere
                        // must not also silence the audio.
                        let mut r = feed.lock().unwrap_or_else(|e| e.into_inner());
                        r.pop(&mut stereo, feed_paused.load(Ordering::Relaxed));
                    }
                    // A trailing partial device frame is zeroed rather than left
                    // alone: cpal hands over the previous callback's memory, so
                    // skipping it replays a fragment of old audio as a tick.
                    let (body, tail) = out.split_at_mut(frames * ch);
                    tail.fill(0.0);
                    // The device's channel count is the device's, and it is not
                    // necessarily two: a mono device takes the left channel, and a
                    // 5.1 device gets L,R and then the pair repeated. Repeating rather
                    // than zeroing the rest, because a player on a surround device
                    // hearing only the front pair would report "no sound" from the
                    // speakers they are facing.
                    for (frame, src) in body
                        .chunks_mut(ch)
                        .zip(stereo.chunks_exact(machine::resample::CHANNELS))
                    {
                        for (i, slot) in frame.iter_mut().enumerate() {
                            *slot = f32::from(src[i % machine::resample::CHANNELS]) / 32768.0;
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
        assert_eq!(
            a.stats(),
            RingStats::default(),
            "nothing plays, so nothing starves"
        );
    }

    /// The null sink says it is not running, and that is what reaches the title bar.
    ///
    /// The one method here whose value is a claim rather than a forward. A sink that
    /// reported `true` would be a window titled as if sound were playing on a machine
    /// with no audio device — and "I hear nothing" is then unattributable between a
    /// missing device, a silent game and a broken mix.
    #[test]
    fn the_null_sink_says_it_is_not_running() {
        let mut a = NullAudio::default();
        assert!(!a.is_running(), "there is no device behind this sink");
        // And a pause is accepted rather than panicking: the loop calls it every tick.
        a.set_paused(true);
        a.set_paused(false);
        assert_eq!(a.stats(), RingStats::default());
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
