//! What happens in the world as commands are accepted: occupancy reports, movement, object
//! commands, and running the triggers that listen for the resulting signals.
use crate::Result;
use crate::tx::Tx;
use crate::*;
use game_types::*;
use std::collections::{BTreeMap, BTreeSet};

/// Signals for what an accepted command changed, however it changed it: a dialogue action,
/// a script and a direct command all announce an acquired item the same way.
pub(crate) fn signals(content: &GameContent, tx: &Tx) -> BTreeSet<WorldSignal> {
    let state: &SessionState = tx;
    let before = &tx.before;
    let mut signals: BTreeSet<WorldSignal> = before.signals.iter().cloned().collect();
    let quantities = |bag: Option<&inventory::Inventory>| {
        let mut totals = BTreeMap::new();
        for entry in bag.into_iter().flat_map(|b| &b.entries) {
            *totals.entry(entry.definition).or_insert(0u64) += u64::from(entry.quantity);
        }
        totals
    };
    for (id, old) in &before.inventories {
        let Some(new) = state.inventories.get(id) else {
            continue;
        };
        if new.owner.kind != "actor" || new.role != CARRIED {
            continue;
        }
        let actor = ActorId(new.owner.id.0);
        let previous = quantities(old.as_ref());
        for (definition, count) in quantities(Some(new)) {
            if count > previous.get(&definition).copied().unwrap_or(0) {
                signals.insert(WorldSignal::ItemAcquired { actor, definition });
            }
        }
    }
    let rules = &content.game.rules;
    for (id, old) in &before.actors {
        let (Some(old), Some(new)) = (old, state.actors.get(id)) else {
            continue;
        };
        if old.alive(rules) && !new.alive(rules) {
            signals.insert(WorldSignal::Died(*id));
        }
        if new.level > old.level {
            signals.insert(WorldSignal::LeveledUp(*id));
        }
    }
    for (id, old) in &before.quests {
        let old = old.clone().unwrap_or_else(|| quests::Progress::new(*id));
        let new = state.quest(*id);
        if old != new {
            signals.insert(WorldSignal::QuestChanged(*id));
            if old.status == quests::Status::NotStarted && new.status == quests::Status::Active {
                signals.insert(WorldSignal::QuestStarted(*id));
            }
        }
    }
    for (key, old) in &before.histories {
        let completed = old.as_ref().map_or(0, |h| h.completed);
        if state.history(*key).completed > completed {
            signals.insert(WorldSignal::DialogueCompleted(key.dialogue));
        }
    }
    for (key, old) in &before.variables {
        if old.as_ref() != state.variables.get(key) {
            signals.insert(WorldSignal::VariableChanged(key.variable));
        }
    }
    for (actor, old) in &before.locations {
        let none = BTreeSet::new();
        let previous = old.as_ref().map_or(&none, |l| &l.areas);
        let current = state.areas(*actor);
        for area in current.difference(previous) {
            signals.insert(WorldSignal::Entered {
                actor: *actor,
                area: *area,
            });
        }
        for area in previous.difference(current) {
            signals.insert(WorldSignal::Exited {
                actor: *actor,
                area: *area,
            });
        }
    }
    signals
}
/// The movement whose time ran out first, if any.
pub(crate) fn timed_out(state: &SessionState) -> Option<ActorId> {
    state
        .world
        .movements
        .values()
        .filter_map(|m| Some((m.deadline?, m.actor)))
        .filter(|(deadline, _)| *deadline <= state.time)
        .min()
        .map(|(_, actor)| actor)
}
/// Starts a movement, or completes it on the spot when the actor is already there.
pub(crate) fn request_move(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    to: AreaId,
    timeout_ms: Option<u64>,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    content.area(to)?;
    crate::character::require_alive(content, tx.actor(actor)?)?;
    if tx.areas(actor).contains(&to) {
        tx.remove_movement(actor);
        arrive(tx, actor, to, events);
        return Ok(());
    }
    // Asking again for a walk that is already under way changes nothing.
    if tx.world.movements.get(&actor).is_some_and(|m| m.to == to) {
        return Ok(());
    }
    let deadline = timeout_ms.map(|ms| tx.time.advance(ms)).transpose()?;
    let movement = Movement {
        actor,
        to,
        // A later request for the same actor replaces this one and gets a larger number.
        request: tx.generation,
        deadline,
    };
    tx.set_movement(movement.clone());
    events.push(GameEvent::World(WorldEvent::MoveRequested(movement)));
    Ok(())
}
fn arrive(tx: &mut Tx, actor: ActorId, area: AreaId, events: &mut Vec<GameEvent>) {
    tx.signal(WorldSignal::Arrived { actor, area });
    events.push(GameEvent::World(WorldEvent::Arrived { actor, area }));
}
fn fail_move(tx: &mut Tx, actor: ActorId, events: &mut Vec<GameEvent>) {
    tx.remove_movement(actor);
    tx.signal(WorldSignal::MoveFailed(actor));
    events.push(GameEvent::World(WorldEvent::MoveFailed { actor }));
}
/// Runs one trigger if it may fire. Its actions take effect together or not at all; a
/// failure is reported and leaves the trigger free to fire another time.
fn run_trigger(content: &GameContent, tx: &mut Tx, id: TriggerId, events: &mut Vec<GameEvent>) {
    let savepoint = tx.savepoint();
    let mark = events.len();
    let result = (|| -> Result<bool> {
        let d = content.trigger(id)?;
        let mut progress = tx.trigger(id);
        if !progress.eligible(d, tx.time) {
            return Ok(false);
        }
        if let Some(condition) = &d.condition
            && !content
                .evaluate(condition, tx, d.player, d.speaker())?
                .matched
        {
            return Ok(false);
        }
        for action in &d.actions {
            let (player, speaker, nobody) = (d.player, d.speaker(), &crate::script::NOBODY);
            crate::session::run_action(content, tx, player, speaker, nobody, action, events)?;
        }
        progress.fired = progress
            .fired
            .checked_add(1)
            .ok_or_else(|| Invalid("trigger count overflow".into()))?;
        progress.last_fired = Some(tx.time);
        tx.put_trigger(progress);
        Ok(true)
    })();
    match result {
        Ok(fired) => {
            tx.release(savepoint);
            if fired {
                events.push(GameEvent::World(WorldEvent::TriggerFired(id)));
            }
        }
        Err(error) => {
            tx.rollback_to(savepoint);
            events.truncate(mark);
            events.push(GameEvent::World(WorldEvent::TriggerFailed {
                trigger: id,
                reason: error.to_string(),
            }));
        }
    }
}
/// Carries out the oldest queued work, or fails a walk whose time ran out.
pub(crate) fn process_next(
    content: &GameContent,
    triggers: &TriggerIndex,
    tx: &mut Tx,
    events: &mut Vec<GameEvent>,
) {
    if let Some(actor) = timed_out(tx) {
        fail_move(tx, actor, events);
        return;
    }
    // The work is taken off the queue first: whatever it leads to, the queue moves on.
    let Some(work) = tx.pending_mut().pop_front() else {
        return;
    };
    match work {
        Pending::Signal(signal) => {
            let ids: Vec<TriggerId> = triggers.subscribers(&signal).collect();
            for id in ids {
                run_trigger(content, tx, id, events);
            }
        }
        Pending::Start {
            dialogue,
            participant,
            speaker,
        } => {
            let savepoint = tx.savepoint();
            let mark = events.len();
            let key = ConversationKey {
                dialogue,
                participant,
                speaker,
            };
            // A graph that cannot be read refuses the conversation like any other reason.
            match crate::conversation::start(content, tx, key, &Default::default(), events) {
                Ok(()) => tx.release(savepoint),
                Err(error) => {
                    tx.rollback_to(savepoint);
                    events.truncate(mark);
                    events.push(GameEvent::World(WorldEvent::DialogueRefused {
                        dialogue,
                        reason: error.to_string(),
                    }));
                }
            }
        }
    }
}
/// Drops the work `process_next` would carry out, when carrying it out failed.
pub(crate) fn drop_next(tx: &mut Tx) {
    match timed_out(tx) {
        Some(actor) => {
            tx.remove_movement(actor);
        }
        None => {
            tx.pending_mut().pop_front();
        }
    }
}
pub(crate) fn apply(
    content: &GameContent,
    tx: &mut Tx,
    command: WorldCommand,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match command {
        WorldCommand::Observe {
            actor,
            position,
            areas,
        } => {
            for area in &areas {
                content.area(*area)?;
            }
            if tx.actor(actor)?.position != position {
                tx.actor_mut(actor)?.position = position;
            }
            if tx.areas(actor) != &areas {
                for area in areas.difference(tx.areas(actor)) {
                    events.push(GameEvent::World(WorldEvent::Entered { actor, area: *area }));
                }
                for area in tx.areas(actor).difference(&areas) {
                    events.push(GameEvent::World(WorldEvent::Exited { actor, area: *area }));
                }
                tx.set_areas(actor, areas);
            }
            if let Some(movement) = tx.world.movements.get(&actor)
                && tx.areas(actor).contains(&movement.to)
            {
                let area = movement.to;
                tx.remove_movement(actor);
                arrive(tx, actor, area, events);
            }
        }
        WorldCommand::Record { positions } => {
            for (actor, position) in positions {
                if tx.actor(actor)?.position != position {
                    tx.actor_mut(actor)?.position = position;
                }
            }
        }
        WorldCommand::MoveFailed { actor, request } => {
            require(
                tx.world
                    .movements
                    .get(&actor)
                    .is_some_and(|m| m.request == request),
                "report is for a movement that is no longer requested",
            )?;
            fail_move(tx, actor, events);
        }
        WorldCommand::Open { object }
        | WorldCommand::Close { object }
        | WorldCommand::SetLocked { object, .. }
        | WorldCommand::Destroy { object } => {
            let o = tx.object_mut(content, object)?;
            Rejection::Destroyed(object).unless(!o.destroyed)?;
            match command {
                WorldCommand::Open { .. } => {
                    Rejection::Locked(object).unless(!o.locked)?;
                    o.open = true;
                    crate::character::fill_container(content, tx, object)?;
                }
                WorldCommand::Close { .. } => o.open = false,
                WorldCommand::SetLocked { locked, .. } => {
                    o.locked = locked;
                    if locked {
                        o.open = false;
                    }
                }
                WorldCommand::Destroy { .. } => {
                    o.destroyed = true;
                    o.open = false;
                }
                _ => unreachable!(),
            }
            events.push(GameEvent::World(WorldEvent::ObjectChanged(object)));
        }
    }
    Ok(())
}
