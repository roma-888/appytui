//! Fallback animation (the sine-plus-noise pattern from AppleMusicTUI) used
//! when audio capture is unavailable.

use super::Frame;

#[derive(Debug, Default)]
pub struct Simulated {
    tick: u64,
}

impl Simulated {
    pub fn frame(&mut self, bars_per_channel: usize, stereo: bool) -> Frame {
        self.tick += 1;
        let t = self.tick as f32;
        let tick = self.tick;
        let bars_for = |offset: f32| -> Vec<f32> {
            (0..bars_per_channel)
                .map(|x| {
                    let xf = x as f32 + offset;
                    let v1 = (xf * 0.8 + t * 0.6).sin();
                    let v2 = (xf * 1.3 - t * 0.4).cos();
                    let noise = ((tick * (x as u64 + 1)) % 5) as f32 * 0.1;
                    ((v1 + v2 + 2.0) / 4.0 + noise).clamp(0.0, 1.0)
                })
                .collect()
        };
        Frame {
            left: bars_for(0.0),
            right: if stereo { bars_for(2.5) } else { Vec::new() },
            waveform: (0..super::spectrum::WAVEFORM_LEN).map(|i| ((i as f32 * 0.1 + t * 0.3).sin()) * 0.6).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_requested_shape_and_animate() {
        let mut s = Simulated::default();
        let a = s.frame(10, true);
        let b = s.frame(10, true);
        assert_eq!(a.left.len(), 10);
        assert_eq!(a.right.len(), 10);
        assert_ne!(a.left, b.left);
        assert!(a.left.iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(s.frame(4, false).right.is_empty());
    }
}
