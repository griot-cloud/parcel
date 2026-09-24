//! Per-caller privacy budget ledger (peQL design 4.8). In-process; one ledger per engine.

use std::collections::HashMap;
use std::sync::Mutex;

/// Default epsilon a caller may spend per named budget.
pub const DEFAULT_LIMIT: f64 = 10.0;

#[derive(Debug, Default)]
pub struct BudgetStore {
    limits: Mutex<HashMap<String, f64>>,
    spent: Mutex<HashMap<(String, String), f64>>,
}

impl BudgetStore {
    pub fn set_limit(&self, budget: &str, epsilon: f64) {
        self.limits
            .lock()
            .expect("budget lock")
            .insert(budget.to_owned(), epsilon);
    }

    pub fn limit(&self, budget: &str) -> f64 {
        self.limits
            .lock()
            .expect("budget lock")
            .get(budget)
            .copied()
            .unwrap_or(DEFAULT_LIMIT)
    }

    /// Charge `epsilon` to a caller's budget; returns what is left, or `None` if it would overdraw.
    pub fn charge(&self, budget: &str, caller: &str, epsilon: f64) -> Option<f64> {
        let limit = self.limit(budget);
        let mut spent = self.spent.lock().expect("budget lock");
        let s = spent
            .entry((budget.to_owned(), caller.to_owned()))
            .or_insert(0.0);
        if *s + epsilon > limit + 1e-12 {
            return None;
        }
        *s += epsilon;
        Some(limit - *s)
    }
}
