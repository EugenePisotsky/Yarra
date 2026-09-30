//! Moves a session through time for the game and for headless runs alike: game time passes
//! in fixed steps, and a bounded trace keeps what the last commands did.
use crate::*;
use game_types::require;
use std::collections::VecDeque;
use std::time::Duration;

/// Real time turned into game time at once at most: a long hitch does not fast-forward the
/// world.
const MAX_ELAPSED: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub generation: u64,
    pub events: Vec<GameEvent>,
}
pub struct Driver<C: ContentSource> {
    session: GameSession<C>,
    step_ms: u64,
    /// Real time not yet turned into game time.
    unspent: Duration,
    trace: VecDeque<TraceEntry>,
    trace_capacity: usize,
}
impl<C: ContentSource> Driver<C> {
    /// A trace capacity of zero keeps no trace.
    pub fn new(session: GameSession<C>, step_ms: u64, trace_capacity: usize) -> Result<Self> {
        require(
            (1..=60_000).contains(&step_ms) && trace_capacity <= 1024,
            "invalid driver step or trace capacity",
        )?;
        Ok(Self {
            session,
            step_ms,
            unspent: Duration::ZERO,
            trace: VecDeque::new(),
            trace_capacity,
        })
    }
    pub fn session(&self) -> &GameSession<C> {
        &self.session
    }
    /// For read models that load a dialogue graph. Commands go through `submit`.
    pub fn session_mut(&mut self) -> &mut GameSession<C> {
        &mut self.session
    }
    pub fn state(&self) -> &SessionState {
        self.session.state()
    }
    pub fn trace(&self) -> &VecDeque<TraceEntry> {
        &self.trace
    }
    pub fn submit(&mut self, command: Command) -> Result<CommandOutcome> {
        let outcome = self.session.apply(command)?;
        if self.trace_capacity > 0 {
            if self.trace.len() == self.trace_capacity {
                self.trace.pop_front();
            }
            self.trace.push_back(TraceEntry {
                generation: outcome.header.generation,
                events: outcome.events.clone(),
            });
        }
        Ok(outcome)
    }
    pub fn container_contents(
        &self,
        object: game_types::ObjectId,
    ) -> Result<&inventory::Inventory> {
        self.session.container_contents(object)
    }
    /// One step of game time.
    pub fn step(&mut self) -> Result<CommandOutcome> {
        self.submit(Command::AdvanceTime {
            millis: self.step_ms,
        })
    }
    /// Lets real time pass. Game time moves on in whole steps; what is left over counts
    /// towards the next call. `None` while less than a step has built up.
    pub fn elapse(&mut self, real: Duration) -> Result<Option<CommandOutcome>> {
        self.unspent += real.min(MAX_ELAPSED);
        let steps = (self.unspent.as_millis() / u128::from(self.step_ms)) as u64;
        if steps == 0 {
            return Ok(None);
        }
        let millis = steps * self.step_ms;
        self.unspent -= Duration::from_millis(millis);
        self.submit(Command::AdvanceTime { millis }).map(Some)
    }
    /// Includes a check before stepping. Timeout is an error; accepted steps remain accepted.
    pub fn advance_until(
        &mut self,
        max_steps: usize,
        predicate: impl Fn(&SessionState) -> bool,
    ) -> Result<usize> {
        require(max_steps <= 100_000, "headless step budget exceeded")?;
        for steps in 0..=max_steps {
            if predicate(self.state()) {
                return Ok(steps);
            }
            if steps < max_steps {
                self.step()?;
            }
        }
        Err(game_types::Invalid(format!(
            "condition not reached in {max_steps} simulation steps"
        ))
        .into())
    }
    /// Carries on with another playthrough, e.g. one just loaded. Real time not yet spent
    /// and the trace go with the old one.
    pub fn replace(&mut self, session: GameSession<C>) {
        self.session = session;
        self.unspent = Duration::ZERO;
        self.trace.clear();
    }
    pub fn into_session(self) -> GameSession<C> {
        self.session
    }
}
