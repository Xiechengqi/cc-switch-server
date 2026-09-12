/// Request-local downstream commit fence. Transparent recovery is legal only
/// before the first business byte/frame is published.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::proxy) struct CommitGuard {
    committed_at_attempt: Option<u32>,
}

impl CommitGuard {
    pub(in crate::proxy) fn is_committed(&self) -> bool {
        self.committed_at_attempt.is_some()
    }

    pub(in crate::proxy) fn commit_business_output(&mut self, attempt: u32) -> bool {
        if self.committed_at_attempt.is_some() {
            return false;
        }
        self.committed_at_attempt = Some(attempt);
        true
    }

    #[cfg(test)]
    pub(in crate::proxy) fn committed_at_attempt(&self) -> Option<u32> {
        self.committed_at_attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_business_output_is_the_only_commit_point() {
        let mut guard = CommitGuard::default();
        assert!(!guard.is_committed());
        assert!(guard.commit_business_output(2));
        assert!(!guard.commit_business_output(3));
        assert_eq!(guard.committed_at_attempt(), Some(2));
    }
}
