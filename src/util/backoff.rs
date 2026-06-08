/// Exponential backoff calculator: each [`Backoff::next`] returns the current
/// delay (in milliseconds) and then doubles it, capped at `max`.
#[derive(Debug, Clone)]
pub(crate) struct Backoff {
    max: u64,
    current: u64,
}

impl Backoff {
    /// Create a new `Backoff` starting at `initial`, capped at `max` (both ms).
    #[must_use]
    pub(crate) const fn new(initial: u64, max: u64) -> Self {
        Self {
            max,
            current: initial,
        }
    }

    /// Return the current delay, then double it (saturating, capped at `max`).
    #[allow(clippy::should_implement_trait)]
    pub(crate) fn next(&mut self) -> u64 {
        let delay = self.current;
        self.current = self.current.saturating_mul(2).min(self.max);
        delay
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(1000, 30000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let backoff = Backoff::new(100, 5000);
        assert_eq!(backoff.max, 5000);
        assert_eq!(backoff.current, 100);
    }

    #[test]
    fn test_default() {
        let backoff = Backoff::default();
        assert_eq!(backoff.max, 30000);
        assert_eq!(backoff.current, 1000);
    }

    #[test]
    fn test_doubling() {
        let mut backoff = Backoff::new(100, 10000);
        assert_eq!(backoff.next(), 100);
        assert_eq!(backoff.next(), 200);
        assert_eq!(backoff.next(), 400);
        assert_eq!(backoff.next(), 800);
        assert_eq!(backoff.next(), 1600);
        assert_eq!(backoff.next(), 3200);
        assert_eq!(backoff.next(), 6400);
    }

    #[test]
    fn test_capping_at_max() {
        let mut backoff = Backoff::new(100, 300);
        assert_eq!(backoff.next(), 100);
        assert_eq!(backoff.next(), 200);
        assert_eq!(backoff.next(), 300); // capped
        assert_eq!(backoff.next(), 300); // stays at max
        assert_eq!(backoff.next(), 300);
    }

    #[test]
    fn test_initial_equals_max() {
        let mut backoff = Backoff::new(500, 500);
        assert_eq!(backoff.next(), 500);
        assert_eq!(backoff.next(), 500);
        assert_eq!(backoff.next(), 500);
    }

    #[test]
    fn test_zero_initial() {
        let mut backoff = Backoff::new(0, 1000);
        assert_eq!(backoff.next(), 0);
        assert_eq!(backoff.next(), 0); // 0 * 2 = 0
    }

    #[test]
    fn test_large_values_no_overflow() {
        let mut backoff = Backoff::new(u64::MAX / 2, u64::MAX);
        assert_eq!(backoff.next(), u64::MAX / 2);
        // saturating_mul prevents overflow: (u64::MAX / 2) * 2 == u64::MAX - 1
        assert_eq!(backoff.next(), u64::MAX - 1);
    }
}
