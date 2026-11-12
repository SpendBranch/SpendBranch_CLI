//! Which model a turn ran on, remembered with a bound.
use std::collections::{HashMap, VecDeque};

/// Turn to model, oldest forgotten first. Bounded so a resume state stays small.
#[derive(Default)]
pub(super) struct TurnModels {
    pub(super) map: HashMap<String, String>,
    order: VecDeque<String>,
}

impl TurnModels {
    const KEEP: usize = 4096;

    pub(super) fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        let mut t = Self::default();
        for (turn, model) in pairs {
            t.insert(turn, model);
        }
        t
    }

    pub(super) fn insert(&mut self, turn: String, model: String) {
        if self.map.insert(turn.clone(), model).is_none() {
            self.order.push_back(turn);
            if self.order.len() > Self::KEEP
                && let Some(old) = self.order.pop_front()
            {
                self.map.remove(&old);
            }
        }
    }

    /// The most recent `RESUME_KEEP` turns, oldest first: what a saved state needs. A response
    /// of an older turn falls back to the model in force when it was written.
    pub(super) fn into_pairs(self) -> Vec<(String, String)> {
        const RESUME_KEEP: usize = 64;
        let skip = self.order.len().saturating_sub(RESUME_KEEP);
        let mut map = self.map;
        self.order
            .into_iter()
            .skip(skip)
            .filter_map(|t| map.remove(&t).map(|m| (t, m)))
            .collect()
    }
}
