use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct DownstreamKeepalive {
    interval: Duration,
    next_at: Option<tokio::time::Instant>,
}

impl DownstreamKeepalive {
    pub(crate) fn new(interval: Duration) -> Option<Self> {
        (!interval.is_zero()).then_some(Self {
            interval,
            next_at: None,
        })
    }

    pub(crate) fn commit(&mut self, now: tokio::time::Instant) {
        self.next_at = Some(now + self.interval);
    }

    pub(crate) fn deadline(&self) -> Option<tokio::time::Instant> {
        self.next_at
    }

    pub(crate) fn emitted(&mut self, now: tokio::time::Instant) {
        self.next_at = Some(now + self.interval);
    }

    #[cfg(test)]
    pub(crate) fn interval(&self) -> Duration {
        self.interval
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remains_disarmed_until_downstream_commit_and_only_advances_itself() {
        let start = tokio::time::Instant::now();
        let mut keepalive = DownstreamKeepalive::new(Duration::from_secs(15)).unwrap();
        assert!(keepalive.deadline().is_none());
        keepalive.commit(start);
        assert_eq!(keepalive.deadline(), Some(start + Duration::from_secs(15)));
        keepalive.emitted(start + Duration::from_secs(15));
        assert_eq!(keepalive.deadline(), Some(start + Duration::from_secs(30)));
    }

    #[test]
    fn zero_disables_keepalive() {
        assert!(DownstreamKeepalive::new(Duration::ZERO).is_none());
    }
}
