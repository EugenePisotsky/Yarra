//! Moves a session through time for the game and for headless runs alike: game time passes
//! in fixed steps.
use crate::{Command, CommandOutcome, GameSession, Result, SessionState, inventory};
use game_types::require;
use std::time::Duration;

/// Real time turned into game time at once at most: a long hitch does not fast-forward the
/// world.
const MAX_ELAPSED: Duration = Duration::from_secs(1);

pub struct Driver {
    session: GameSession,
    step_ms: u64,
    /// Real time not yet turned into game time.
    unspent: Duration,
}

impl Driver {
    pub fn new(session: GameSession, step_ms: u64) -> Result<Self> {
        require((1..=60_000).contains(&step_ms), "invalid driver step")?;
        Ok(Self {
            session,
            step_ms,
            unspent: Duration::ZERO,
        })
    }

    pub fn session(&self) -> &GameSession {
        &self.session
    }

    pub fn state(&self) -> &SessionState {
        self.session.state()
    }

    pub fn submit(&mut self, command: Command) -> Result<CommandOutcome> {
        self.session.apply(command)
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
    /// goes with the old one.
    pub fn replace(&mut self, session: GameSession) {
        self.session = session;
        self.unspent = Duration::ZERO;
    }
}
