//! One domain's saves through the project writer: the request waiting to be sent, the one the
//! writer has, and its answer until the domain takes it. The store sends one save at a time.

pub(super) struct SaveChannel<Request, Answer> {
    pending: Option<(u64, Request)>,
    in_flight: Option<u64>,
    answer: Option<(u64, Answer)>,
}

impl<Request, Answer> Default for SaveChannel<Request, Answer> {
    fn default() -> Self {
        Self {
            pending: None,
            in_flight: None,
            answer: None,
        }
    }
}

impl<Request: Clone, Answer> SaveChannel<Request, Answer> {
    /// A request waits or the writer has one.
    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.in_flight.is_some()
    }

    pub(super) fn in_flight(&self) -> bool {
        self.in_flight.is_some()
    }

    pub(super) fn queue(&mut self, id: u64, request: Request) {
        self.pending = Some((id, request));
    }

    /// The request waiting to be sent, if any.
    pub(super) fn pending(&self) -> Option<(u64, Request)> {
        self.pending.clone()
    }

    /// The waiting request reached the writer.
    pub(super) fn sent(&mut self) {
        if let Some((id, _)) = self.pending.take() {
            self.in_flight = Some(id);
        }
    }

    /// The writer answered request `id`; true when it was the one in flight.
    pub(super) fn answered(&mut self, id: u64) -> bool {
        let current = self.in_flight == Some(id);

        if current {
            self.in_flight = None;
        }

        current
    }

    pub(super) fn complete(&mut self, id: u64, answer: Answer) {
        self.answer = Some((id, answer));
    }

    /// The last answer, for the domain that asked.
    pub(super) fn take(&mut self) -> Option<(u64, Answer)> {
        self.answer.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_waits_is_sent_and_answered_once() {
        let mut channel = SaveChannel::<&str, bool>::default();
        assert!(!channel.busy());
        channel.queue(7, "writes");
        assert!(channel.busy() && !channel.in_flight());
        assert_eq!(channel.pending(), Some((7, "writes")));
        channel.sent();
        assert!(channel.in_flight() && channel.pending().is_none());
        assert!(!channel.answered(6), "an older request's answer");
        assert!(channel.answered(7));
        assert!(!channel.busy());
        channel.complete(7, true);
        assert_eq!(channel.take(), Some((7, true)));
        assert_eq!(channel.take(), None);
    }
}
