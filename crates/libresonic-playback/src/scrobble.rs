//! Listening-time policy. Seeking and paused/buffering time earn no credit.
use libresonic_core::MediaId;
use serde::{Deserialize, Serialize};

/// User-controlled reporting policy; disabled by default.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScrobbleSettings {
    /// Send now-playing and completed-listen notifications.
    pub enabled: bool,
    /// Percentage of track duration that must actually be heard (1..=100).
    pub percent: u8,
    /// Optional alternative listened-seconds threshold; zero disables this cap.
    pub seconds: u32,
}
impl Default for ScrobbleSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            percent: 50,
            seconds: 240,
        }
    }
}
impl ScrobbleSettings {
    /// Time needed: percentage or optional seconds, whichever comes first.
    pub fn threshold_ms(&self, duration_ms: Option<u64>) -> Option<u64> {
        let percentage = duration_ms.filter(|d| *d > 0).map(|d| {
            d.saturating_mul(u64::from(self.percent.clamp(1, 100)))
                .div_ceil(100)
        });
        let seconds = (self.seconds > 0).then_some(u64::from(self.seconds.clamp(1, 86400)) * 1000);
        match (percentage, seconds) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}
/// One non-replayed report, keyed by a playback occurrence and report kind.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scrobble {
    /// Original track identity.
    pub track: MediaId,
    /// Random occurrence identity; repeated songs get different values.
    pub occurrence: u128,
    /// UTC start time in milliseconds.
    pub started_ms: u64,
    /// False is now playing; true is a threshold-qualified listen.
    pub submission: bool,
}
/// Serializable current occurrence. The caller checkpoints before reporting.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Listening {
    current: Option<Scrobble>,
    heard_ms: u64,
    notified: bool,
    submitted: bool,
    #[serde(skip)]
    previous: Option<(u64, u64, bool)>,
}
impl Listening {
    /// Begin a new occurrence, even when the identity equals the previous song.
    pub fn start(&mut self, track: MediaId, started_ms: u64) {
        *self = Self {
            current: Some(Scrobble {
                track,
                occurrence: rand::random(),
                started_ms,
                submission: false,
            }),
            ..Self::default()
        };
    }
    /// Stop credit accumulation; preserve prior reporting decisions.
    pub fn suspend(&mut self) {
        self.previous = None;
    }
    /// Listen to one player sample. `elapsed_ms` bounds credited position changes.
    /// `generation` changes on seeks. Non-playing samples reset accumulation.
    pub fn observe(
        &mut self,
        settings: &ScrobbleSettings,
        position_ms: u64,
        duration_ms: Option<u64>,
        generation: u64,
        playing: bool,
        elapsed_ms: u64,
    ) -> Vec<Scrobble> {
        let mut reports = Vec::new();
        if !settings.enabled {
            self.previous = None;
            return reports;
        }
        let Some(current) = &self.current else {
            return reports;
        };
        if let Some((position, old_generation, was_playing)) = self.previous
            && (was_playing || playing)
            && old_generation == generation
        {
            self.heard_ms = self.heard_ms.saturating_add(
                position_ms
                    .saturating_sub(position)
                    .min(elapsed_ms.saturating_add(50)),
            );
        }
        self.previous = Some((position_ms, generation, playing));
        if playing && !self.notified {
            self.notified = true;
            reports.push(current.clone());
        }
        if !self.submitted
            && settings
                .threshold_ms(duration_ms)
                .is_some_and(|threshold| self.heard_ms >= threshold)
        {
            self.submitted = true;
            reports.push(Scrobble {
                submission: true,
                ..current.clone()
            });
        }
        reports
    }
    /// Current occurrence identity.
    pub fn track(&self) -> Option<&MediaId> {
        self.current.as_ref().map(|r| &r.track)
    }
    /// Accumulated heard time, excluding transport skips and paused time.
    pub fn heard_ms(&self) -> u64 {
        self.heard_ms
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn id() -> MediaId {
        MediaId::new(
            libresonic_core::ServerId::new("test").unwrap(),
            libresonic_core::MediaKind::Track,
            "song",
        )
        .unwrap()
    }
    #[test]
    fn threshold_pause_seek_disable_and_repeats_are_distinct() {
        let mut settings = ScrobbleSettings {
            enabled: true,
            percent: 50,
            seconds: 0,
        };
        let mut l = Listening::default();
        l.start(id(), 10);
        assert_eq!(l.observe(&settings, 0, Some(10_000), 1, true, 0).len(), 1);
        l.observe(&settings, 2000, Some(10_000), 1, false, 2000);
        l.observe(&settings, 2000, Some(10_000), 1, true, 20000);
        l.observe(&settings, 9000, Some(10_000), 2, true, 1);
        assert_eq!(l.heard_ms(), 2000);
        l.observe(&settings, 10000, Some(10_000), 2, false, 1000);
        settings.enabled = false;
        l.observe(&settings, 0, Some(10_000), 3, true, 1000);
        settings.enabled = true;
        l.observe(&settings, 0, Some(10_000), 3, true, 1000);
        let submitted = l.observe(&settings, 2000, Some(10_000), 3, true, 2000);
        assert_eq!(submitted.len(), 1);
        assert!(submitted[0].submission);
        assert!(
            l.observe(&settings, 4000, Some(10_000), 3, true, 2000)
                .is_empty()
        );
        let old = submitted[0].occurrence;
        l.start(id(), 20);
        assert_ne!(
            l.observe(&settings, 0, Some(10_000), 4, true, 0)[0].occurrence,
            old
        );
    }
    #[test]
    fn unknown_duration_caps_and_restart_do_not_invent_listening() {
        let settings = ScrobbleSettings {
            enabled: true,
            percent: 50,
            seconds: 2,
        };
        assert_eq!(settings.threshold_ms(Some(10_000)), Some(2000));
        assert_eq!(settings.threshold_ms(None), Some(2000));
        assert_eq!(
            ScrobbleSettings {
                seconds: 0,
                ..settings.clone()
            }
            .threshold_ms(None),
            None
        );
        let mut l = Listening::default();
        l.start(id(), 1);
        l.observe(&settings, 0, None, 1, true, 0);
        let mut restored = l.clone();
        restored.suspend();
        assert!(
            restored
                .observe(&settings, 100000, None, 2, false, 20000)
                .is_empty()
        );
        assert_eq!(restored.heard_ms(), 0);
    }
}
