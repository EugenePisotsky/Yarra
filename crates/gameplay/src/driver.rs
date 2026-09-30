//! The same commands and read models used by application adapters, with an explicit test clock.
use crate::*;
use game_types::require;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub generation: u64,
    pub events: Vec<GameEvent>,
}
pub struct HeadlessDriver<C: ContentSource> {
    session: GameSession<C>,
    step_ms: u64,
    trace: VecDeque<TraceEntry>,
    trace_capacity: usize,
}
impl<C: ContentSource> HeadlessDriver<C> {
    pub fn new(session: GameSession<C>, step_ms: u64, trace_capacity: usize) -> Result<Self> {
        require(
            (1..=60_000).contains(&step_ms) && (1..=1024).contains(&trace_capacity),
            "invalid headless driver budgets",
        )?;
        Ok(Self {
            session,
            step_ms,
            trace: VecDeque::new(),
            trace_capacity,
        })
    }
    pub fn session(&self) -> &GameSession<C> {
        &self.session
    }
    pub fn state(&self) -> &SessionState {
        self.session.state()
    }
    pub fn trace(&self) -> &VecDeque<TraceEntry> {
        &self.trace
    }
    pub fn submit(&mut self, command: Command) -> Result<CommandOutcome> {
        let outcome = self.session.apply(command)?;
        if self.trace.len() == self.trace_capacity {
            self.trace.pop_front();
        }
        self.trace.push_back(TraceEntry {
            generation: outcome.header.generation,
            events: outcome.events.clone(),
        });
        Ok(outcome)
    }
    /// Drain a bounded number of committed deliveries; false means work remains for a later tick.
    pub fn pump_world(&mut self, max_deliveries: usize) -> Result<bool> {
        require(max_deliveries <= 10000, "headless delivery budget exceeded")?;
        for _ in 0..max_deliveries {
            if !self.session.world_work_pending() {
                return Ok(true);
            }
            self.submit(Command::World(WorldCommand::ProcessNext))?;
        }
        Ok(!self.session.world_work_pending())
    }
    pub fn next_movement(&self, after: Option<game_types::TriggerId>) -> Option<MoveRequest> {
        self.session.next_movement(after)
    }
    pub fn container_contents(
        &self,
        object: game_types::ObjectId,
    ) -> Result<&inventory::Inventory> {
        self.session.container_contents(object)
    }
    pub fn step(&mut self) -> Result<CommandOutcome> {
        self.submit(Command::AdvanceTime {
            millis: self.step_ms,
        })
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
    pub fn into_session(self) -> GameSession<C> {
        self.session
    }
}
