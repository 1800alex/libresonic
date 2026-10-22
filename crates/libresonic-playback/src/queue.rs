//! Local queue policy, independent of devices, protocols and views.
use libresonic_core::{MediaId, Track};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};

/// Repeat applies to natural ends; an explicit Next always advances.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    /// Stop at the context end.
    #[default]
    Off,
    /// Start the context again after its final song.
    Context,
    /// Replay the current occurrence at natural end.
    Track,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Occurrence {
    track: Track,
    index: Option<usize>,
}

/// Serializable application-owned queue. Repeated IDs remain separate occurrences.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    context: Vec<Track>,
    order: Vec<usize>,
    next: usize,
    current: Option<Track>,
    current_index: Option<usize>,
    manual: Vec<Track>,
    history: Vec<Occurrence>,
    forward: Vec<Occurrence>,
    /// Scoped source collection, when the selection has one.
    pub source: Option<MediaId>,
    /// Shuffle future context occurrences without moving the current song.
    pub shuffle: bool,
    /// Local repeat preference.
    pub repeat: Repeat,
}

impl Queue {
    /// Start exactly the selected occurrence, preserving manually queued songs and modes.
    pub fn start(
        &mut self,
        tracks: Vec<Track>,
        index: usize,
        source: Option<MediaId>,
    ) -> Option<Track> {
        let current = tracks.get(index)?.clone();
        self.context = tracks;
        self.order = (0..self.context.len()).collect();
        if self.shuffle {
            self.order.remove(index);
            self.order.shuffle(&mut rand::rng());
            self.order.insert(0, index);
            self.next = 1;
        } else {
            self.next = index + 1;
        }
        self.current = Some(current.clone());
        self.current_index = Some(index);
        self.source = source;
        self.history.clear();
        self.forward.clear();
        Some(current)
    }
    /// Current selected occurrence.
    pub fn current(&self) -> Option<&Track> {
        self.current.as_ref()
    }
    /// Original context occurrence currently selected; None for manual entries.
    pub fn current_index(&self) -> Option<usize> {
        self.current_index
    }
    /// Manually queued occurrences, in play order.
    pub fn manual(&self) -> &[Track] {
        &self.manual
    }
    /// Visible future occurrences: manual songs first, then context/history continuation.
    pub fn upcoming(&self) -> Vec<Track> {
        self.manual
            .iter()
            .cloned()
            .chain(self.forward.iter().rev().map(|entry| entry.track.clone()))
            .chain(
                self.order
                    .iter()
                    .skip(self.next)
                    .filter_map(|i| self.context.get(*i))
                    .cloned(),
            )
            .collect()
    }
    /// Append occurrences, including duplicates.
    pub fn add(&mut self, tracks: Vec<Track>) {
        self.manual.extend(tracks);
    }
    /// Insert occurrences into the manual section.
    pub fn insert(&mut self, at: usize, tracks: Vec<Track>) {
        let at = at.min(self.manual.len());
        self.manual.splice(at..at, tracks);
    }
    /// Remove only a matching manual occurrence, rejecting stale row identities.
    pub fn remove(&mut self, at: usize, expected: &MediaId) {
        if self.manual.get(at).is_some_and(|t| &t.id == expected) {
            self.manual.remove(at);
        }
    }
    /// Move one manual row to a drop slot in that section.
    pub fn move_row(&mut self, from: usize, to: usize) {
        if from >= self.manual.len() || to > self.manual.len() {
            return;
        }
        let track = self.manual.remove(from);
        self.manual
            .insert(if to > from { to - 1 } else { to }, track);
    }
    /// Clear only manually queued occurrences.
    pub fn clear(&mut self) {
        self.manual.clear();
    }
    /// Reorder future context occurrences without changing manually queued songs/current/history.
    pub fn set_shuffle(&mut self, shuffle: bool) {
        self.shuffle = shuffle;
        let Some(tail) = self.order.get_mut(self.next..) else {
            return;
        };
        if shuffle {
            tail.shuffle(&mut rand::rng());
        } else {
            tail.sort_unstable();
        }
    }
    /// Advance, consuming exactly one occurrence. Errors do not call this automatically.
    pub fn advance(&mut self, natural: bool) -> Option<Track> {
        if natural && self.repeat == Repeat::Track {
            return self.current.clone();
        }
        let selected = if !self.manual.is_empty() {
            Some(Occurrence {
                track: self.manual.remove(0),
                index: None,
            })
        } else if let Some(entry) = self.forward.pop() {
            Some(entry)
        } else {
            if self.next >= self.order.len() && self.repeat == Repeat::Context {
                self.next = 0;
                if self.shuffle {
                    self.order.shuffle(&mut rand::rng());
                }
            }
            let selected = self.order.get(self.next).and_then(|i| {
                self.context.get(*i).map(|track| Occurrence {
                    track: track.clone(),
                    index: Some(*i),
                })
            });
            if selected.is_some() {
                self.next += 1;
            }
            selected
        };
        if let Some(track) = self.current.take() {
            if self.history.len() == 256 {
                self.history.remove(0);
            }
            self.history.push(Occurrence {
                track,
                index: self.current_index.take(),
            });
        }
        self.current_index = selected.as_ref().and_then(|entry| entry.index);
        self.current = selected.map(|entry| entry.track);
        self.current.clone()
    }
    /// Previous history occurrence; Next returns to the occurrence left behind.
    pub fn previous(&mut self) -> Option<Track> {
        let previous = self.history.pop()?;
        if let Some(current) = self.current.replace(previous.track.clone()) {
            self.forward.push(Occurrence {
                track: current,
                index: self.current_index.take(),
            });
        }
        self.current_index = previous.index;
        Some(previous.track)
    }
    /// Play an upcoming occurrence, consuming the rows before it.
    pub fn skip_to(&mut self, index: usize) -> Option<Track> {
        if index >= self.upcoming().len() {
            return None;
        }
        let mut selected = None;
        for _ in 0..=index {
            selected = self.advance(false);
            if selected.is_none() {
                break;
            }
        }
        selected
    }
    /// Validate a restored snapshot before it can drive requests or indexing.
    pub fn valid_for(&self, server: &libresonic_core::ServerId) -> bool {
        self.current_index.is_none_or(|i| {
            self.context
                .get(i)
                .is_some_and(|t| Some(&t.id) == self.current.as_ref().map(|t| &t.id))
        }) && self.history.iter().chain(&self.forward).all(|e| {
            e.index
                .is_none_or(|i| self.context.get(i).is_some_and(|t| t.id == e.track.id))
        }) && self.next <= self.order.len()
            && self.order.len() == self.context.len()
            && self.order.iter().all(|i| *i < self.context.len())
            && {
                let mut order = self.order.clone();
                order.sort_unstable();
                order == (0..self.context.len()).collect::<Vec<_>>()
            }
            && self
                .context
                .iter()
                .chain(&self.manual)
                .chain(self.history.iter().map(|e| &e.track))
                .chain(self.forward.iter().map(|e| &e.track))
                .chain(self.current.iter())
                .all(|t| {
                    t.id.server() == server && t.id.kind() == libresonic_core::MediaKind::Track
                })
            && self.source.as_ref().is_none_or(|id| id.server() == server)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libresonic_core::{MediaKind, ServerId};
    fn track(name: &str) -> Track {
        Track {
            id: MediaId::new(ServerId::new("one").unwrap(), MediaKind::Track, name).unwrap(),
            title: name.into(),
            artists: vec![],
            album: None,
            duration_ms: Some(3000),
            track_number: None,
            disc_number: None,
            content_type: None,
            suffix: Some("mp3".into()),
            artwork: None,
            favorite: false,
            rating: None,
            file_path: None,
            file_size: None,
            play_count: None,
        }
    }
    #[test]
    fn manual_copies_precede_context_and_survive_context_replacement() {
        let mut q = Queue::default();
        q.start(vec![track("a"), track("b")], 0, None);
        q.add(vec![track("x"), track("x")]);
        q.start(vec![track("c"), track("d")], 0, None);
        assert_eq!(q.advance(false).unwrap().title, "x");
        assert_eq!(q.advance(false).unwrap().title, "x");
        assert_eq!(q.advance(false).unwrap().title, "d");
        assert!(q.advance(false).is_none());
    }
    #[test]
    fn selected_duplicate_occurrence_next_previous_and_clear_are_exact() {
        let mut q = Queue::default();
        q.start(
            vec![track("a"), track("b"), track("a"), track("c")],
            2,
            None,
        );
        q.add(vec![track("x"), track("x")]);
        q.remove(0, &track("wrong").id);
        assert_eq!(q.manual.len(), 2);
        q.clear();
        assert_eq!(q.advance(false).unwrap().title, "c");
        assert_eq!(q.previous().unwrap().title, "a");
        assert_eq!(q.current_index(), Some(2));
        assert_eq!(q.advance(false).unwrap().title, "c");
    }
    #[test]
    fn repeat_track_only_repeats_natural_end_and_context_wraps() {
        let mut q = Queue::default();
        q.start(vec![track("a"), track("b")], 0, None);
        q.repeat = Repeat::Track;
        assert_eq!(q.advance(true).unwrap().title, "a");
        assert_eq!(q.advance(false).unwrap().title, "b");
        q.repeat = Repeat::Context;
        assert_eq!(q.advance(true).unwrap().title, "a");
    }
    #[test]
    fn shuffle_and_snapshot_preserve_occurrences_namespace_and_manual_order() {
        let mut q = Queue::default();
        q.start(
            vec![track("a"), track("b"), track("b"), track("c")],
            0,
            None,
        );
        q.add(vec![track("x"), track("y")]);
        q.set_shuffle(true);
        assert_eq!(q.current().unwrap().title, "a");
        q.set_shuffle(false);
        assert_eq!(
            q.upcoming()
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["x", "y", "b", "b", "c"]
        );
        assert!(q.valid_for(&ServerId::new("one").unwrap()));
        assert!(!q.valid_for(&ServerId::new("other").unwrap()));
        q.order[1] = 999;
        assert!(!q.valid_for(&ServerId::new("one").unwrap()));
    }
    #[test]
    fn skip_insert_remove_and_move_consume_only_the_chosen_occurrence() {
        let mut q = Queue::default();
        q.start(vec![track("a"), track("b")], 0, None);
        q.add(vec![track("x"), track("y"), track("x")]);
        q.move_row(2, 0);
        q.remove(1, &track("x").id);
        q.insert(1, vec![track("z")]);
        assert_eq!(q.skip_to(1).unwrap().title, "z");
        assert_eq!(
            q.manual()
                .iter()
                .map(|t| t.title.as_str())
                .collect::<Vec<_>>(),
            ["y"]
        );
        assert_eq!(q.advance(true).unwrap().title, "y");
        assert_eq!(q.advance(true).unwrap().title, "b");
    }
}
