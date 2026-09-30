//! Trigger dispatch, movement sequences and object commands.
use crate::Result;
use crate::tx::Tx;
use crate::*;
use game_types::*;
use std::collections::{BTreeMap, BTreeSet};

/// Notifications derived from what an accepted command actually changed, including changes
/// made by dialogue actions and trigger sequences.
pub(crate) fn signals(content: &GameContent, tx: &Tx) -> Result<BTreeSet<WorldSignal>> {
    let state: &SessionState = tx;
    let before = &tx.before;
    let mut signals = BTreeSet::new();
    let quantities = |bag: Option<&inventory::Inventory>| {
        let mut totals = BTreeMap::new();
        for entry in bag.into_iter().flat_map(|b| &b.entries) {
            *totals.entry(entry.definition).or_insert(0u64) += u64::from(entry.quantity);
        }
        totals
    };
    for (id, old) in &before.inventories {
        let new = state.inventories.get(id);
        let Some(owner) = new.or(old.as_ref()).map(|i| &i.owner) else {
            continue;
        };
        if owner.kind != "actor" || old.as_ref() == new {
            continue;
        }
        let actor = ActorId(owner.id.0);
        signals.insert(WorldSignal::Actor(actor));
        if new.is_some_and(|i| i.role == CARRIED) {
            let previous = quantities(old.as_ref());
            for (definition, count) in quantities(new) {
                if count > previous.get(&definition).copied().unwrap_or(0) {
                    signals.insert(WorldSignal::ItemAcquired { actor, definition });
                }
            }
        }
    }
    for (id, old) in &before.actors {
        if old.as_ref() != state.actors.get(id) {
            signals.insert(WorldSignal::Actor(*id));
        }
    }
    for (id, old) in &before.quests {
        let old = old.clone().unwrap_or_else(|| quests::Progress::new(*id));
        let new = state.quest(*id);
        if old != new {
            signals.insert(WorldSignal::Quest(*id));
            if old.status == quests::Status::NotStarted && new.status == quests::Status::Active {
                signals.insert(WorldSignal::QuestStarted(*id));
            }
        }
    }
    for (key, old) in &before.histories {
        if old.as_ref() != state.histories.get(key) {
            signals.insert(WorldSignal::History(*key));
        }
    }
    for key in &before.claims {
        signals.insert(WorldSignal::Claim(*key));
    }
    for (key, old) in &before.relationships {
        let old = old
            .clone()
            .unwrap_or_else(|| actors::Relationship::neutral(*key));
        if old != state.relationship(*key) {
            signals.insert(WorldSignal::Relationship(*key));
        }
    }
    for (id, old) in &before.variables {
        if old.as_ref() != state.variables.get(id) {
            signals.insert(WorldSignal::Variable(*id));
        }
    }
    for (actor, old) in &before.locations {
        // Before its first record, occupancy followed the actor's position at that time.
        let previous = match old {
            Some(location) => location.areas.clone(),
            None => {
                let position = match before.actors.get(actor) {
                    Some(Some(old)) => &old.position,
                    _ => &state.actor(*actor)?.position,
                };
                content.areas_at(position)?
            }
        };
        let current = state.location(content, *actor)?.areas;
        for area in current.difference(&previous) {
            signals.insert(WorldSignal::Entered {
                actor: *actor,
                area: *area,
            });
        }
        for area in previous.difference(&current) {
            signals.insert(WorldSignal::Exited {
                actor: *actor,
                area: *area,
            });
        }
    }
    Ok(signals)
}
/// The pending movement whose deadline passed first, if any.
pub(crate) fn timed_out(state: &SessionState) -> Option<TriggerId> {
    state
        .world
        .triggers
        .values()
        .filter_map(|t| Some((t.movement.as_ref()?.deadline, t.id)))
        .filter(|(deadline, _)| *deadline <= state.time)
        .min()
        .map(|(_, id)| id)
}
fn position(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    p: actors::Position,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let areas = content.areas_at(&p)?;
    let loc = tx.location_mut(content, actor)?;
    for area in areas.difference(&loc.areas) {
        events.push(GameEvent::World(WorldEvent::Entered { actor, area: *area }));
    }
    for area in loc.areas.difference(&areas) {
        events.push(GameEvent::World(WorldEvent::Exited { actor, area: *area }));
    }
    loc.areas = areas;
    tx.actor_mut(actor)?.position = p;
    Ok(())
}
fn sequence(
    content: &GameContent,
    tx: &mut Tx,
    mut progress: TriggerState,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let d = content.trigger(progress.id)?;
    while let Some(step) = d.steps.get(usize::from(progress.step)) {
        match step {
            SequenceStep::Apply(actions) => {
                for action in actions {
                    crate::session::run_action(
                        content,
                        tx,
                        d.participant,
                        d.speaker,
                        action,
                        events,
                    )?;
                }
                progress.step += 1;
            }
            SequenceStep::Move {
                actor,
                destination,
                timeout_ms,
            } => {
                let actor = actor.resolve(d.participant, d.speaker);
                require(
                    tx.movement_owner(actor)
                        .is_none_or(|owner| owner == progress.id),
                    "actor movement is owned by another sequence",
                )?;
                require(
                    tx.actor(actor)?.health > 0,
                    "dead actor cannot accept movement",
                )?;
                let movement = MoveRequest {
                    id: ActionId {
                        trigger: progress.id,
                        run: progress.run,
                        step: progress.step,
                    },
                    actor,
                    destination: destination.clone(),
                    deadline: tx.time.advance(*timeout_ms)?,
                    phase: MovementPhase::Accepted,
                };
                events.push(GameEvent::World(WorldEvent::MoveRequested(
                    movement.clone(),
                )));
                progress.movement = Some(movement);
                break;
            }
        }
    }
    if usize::from(progress.step) == d.steps.len() {
        progress.consecutive_failures = 0;
        progress.diagnostic = None;
        progress.status = SequenceStatus::Succeeded;
        progress.successes = progress
            .successes
            .checked_add(1)
            .ok_or_else(|| Invalid("sequence success overflow".into()))?;
    }
    events.push(GameEvent::World(WorldEvent::SequenceChanged {
        trigger: progress.id,
        status: progress.status.clone(),
    }));
    tx.put_trigger(progress);
    Ok(())
}

/// A plan that fails is undone on its own and recorded with a bounded diagnostic: it consumes
/// no claims and leaves no partial effects, while earlier work of the command is kept.
fn run_sequence(
    content: &GameContent,
    tx: &mut Tx,
    mut progress: TriggerState,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let savepoint = tx.savepoint();
    let event_count = events.len();
    match sequence(content, tx, progress.clone(), events) {
        Ok(()) => {
            tx.release(savepoint);
            Ok(())
        }
        Err(error @ GameplayError::Runtime(_)) => {
            tx.release(savepoint);
            Err(error)
        }
        Err(error) => {
            tx.rollback_to(savepoint);
            events.truncate(event_count);
            progress.status = SequenceStatus::Failed {
                reason: Key::new("invalid-plan")?,
            };
            progress.consecutive_failures = progress.consecutive_failures.saturating_add(1).min(3);
            progress.diagnostic = Some(error.to_string().chars().take(512).collect());
            progress.movement = None;
            events.push(GameEvent::World(WorldEvent::SequenceChanged {
                trigger: progress.id,
                status: progress.status.clone(),
            }));
            tx.put_trigger(progress);
            Ok(())
        }
    }
}
fn restart(progress: &mut TriggerState, now: GameTime) -> Result<()> {
    progress.run = progress
        .run
        .checked_add(1)
        .ok_or_else(|| Invalid("sequence run overflow".into()))?;
    progress.step = 0;
    progress.status = SequenceStatus::Running;
    progress.last_started = Some(now);
    progress.receipts.clear();
    progress.diagnostic = None;
    Ok(())
}
fn complete(
    content: &GameContent,
    tx: &mut Tx,
    id: ActionId,
    result: MoveResult,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    content.trigger(id.trigger)?;
    let mut progress = tx.trigger(id.trigger);
    if let Some((_, receipt)) = progress.receipts.iter().find(|(a, _)| *a == id) {
        require(receipt == &result, "conflicting movement result")?;
        return Ok(());
    }
    let pending = progress
        .movement
        .clone()
        .ok_or_else(|| Invalid("movement is not pending".into()))?;
    require(pending.id == id, "stale movement result")?;
    if !matches!(result, MoveResult::TimedOut) {
        require(
            tx.time < pending.deadline,
            "movement deadline elapsed; process timeout first",
        )?;
    }
    match &result {
        MoveResult::Arrived { position: p } => {
            require(
                p == &pending.destination,
                "arrival differs from requested destination",
            )?;
            require(
                tx.actor(pending.actor)?.health > 0,
                "dead actor cannot arrive",
            )?;
            position(content, tx, pending.actor, p.clone(), events)?;
            progress.step += 1;
        }
        MoveResult::Failed { reason } => {
            progress.status = SequenceStatus::Failed {
                reason: reason.clone(),
            }
        }
        MoveResult::Cancelled => progress.status = SequenceStatus::Cancelled,
        MoveResult::TimedOut => {
            require(tx.time >= pending.deadline, "movement has not timed out")?;
            progress.status = SequenceStatus::Failed {
                reason: Key::new("timeout")?,
            };
        }
    }
    if let SequenceStatus::Failed { reason } = &progress.status {
        progress.consecutive_failures = progress.consecutive_failures.saturating_add(1).min(3);
        progress.diagnostic = Some(reason.as_str().to_owned());
    }
    progress.movement = None;
    progress.receipts.push((id, result.clone()));
    events.push(GameEvent::World(WorldEvent::MoveFinished { id, result }));
    if progress.status == SequenceStatus::Running {
        run_sequence(content, tx, progress, events)
    } else {
        events.push(GameEvent::World(WorldEvent::SequenceChanged {
            trigger: progress.id,
            status: progress.status.clone(),
        }));
        tx.put_trigger(progress);
        Ok(())
    }
}
pub(crate) fn apply(
    content: &GameContent,
    triggers: &TriggerIndex,
    tx: &mut Tx,
    command: WorldCommand,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match command {
        WorldCommand::ProcessNext => {
            if let Some(id) = timed_out(tx) {
                let action = tx
                    .trigger(id)
                    .movement
                    .ok_or_else(|| Invalid("movement index mismatch".into()))?
                    .id;
                return complete(content, tx, action, MoveResult::TimedOut, events);
            }
            let Some(event) = tx.world.pending.front().cloned() else {
                return Ok(());
            };
            let candidate = triggers.next(&event.signal, event.after);
            if let Some(id) = candidate {
                let d = content.trigger(id)?;
                let mut progress = tx.trigger(id);
                if progress.eligible(d, tx.time)
                    && content
                        .evaluate(&d.condition, tx, d.participant, d.speaker)?
                        .matched
                {
                    restart(&mut progress, tx.time)?;
                    run_sequence(content, tx, progress, events)?;
                }
            }
            // The event stays at the head until every subscriber has been visited.
            let pending = tx.pending_mut();
            pending.pop_front();
            if let Some(id) = candidate {
                pending.push_front(PendingEvent {
                    after: Some(id),
                    ..event.clone()
                });
            }
            events.push(GameEvent::World(WorldEvent::DeliveryProcessed(event.id)));
        }
        WorldCommand::RetryTrigger { trigger } => {
            let d = content.trigger(trigger)?;
            let mut progress = tx.trigger(trigger);
            require(
                matches!(
                    progress.status,
                    SequenceStatus::Failed { .. } | SequenceStatus::Cancelled
                ),
                "only failed or cancelled sequences can retry",
            )?;
            progress.consecutive_failures = 0;
            require(
                progress.eligible(d, tx.time)
                    && content
                        .evaluate(&d.condition, tx, d.participant, d.speaker)?
                        .matched,
                "trigger retry is ineligible",
            )?;
            restart(&mut progress, tx.time)?;
            run_sequence(content, tx, progress, events)?;
        }
        WorldCommand::ObservePosition {
            actor,
            observation,
            position: p,
        } => {
            let loc = tx.location(content, actor)?;
            if observation == loc.observation {
                require(
                    loc.last_observed.as_ref() == Some(&p),
                    "conflicting position observation",
                )?;
                return Ok(());
            }
            require(
                observation
                    == loc
                        .observation
                        .checked_add(1)
                        .ok_or_else(|| Invalid("observation overflow".into()))?,
                "out-of-order position observation",
            )?;
            let loc = tx.location_mut(content, actor)?;
            loc.observation = observation;
            loc.last_observed = Some(p.clone());
            position(content, tx, actor, p, events)?;
        }
        WorldCommand::FinishMove { id, result } => complete(content, tx, id, result, events)?,
        WorldCommand::StartMove { id } => {
            content.trigger(id.trigger)?;
            let mut progress = tx.trigger(id.trigger);
            if progress.receipts.iter().any(|(a, _)| *a == id) {
                return Ok(());
            }
            let m = progress
                .movement
                .as_mut()
                .ok_or_else(|| Invalid("movement not pending".into()))?;
            require(m.id == id && tx.time < m.deadline, "stale movement start")?;
            m.phase = MovementPhase::Running;
            tx.put_trigger(progress);
        }
        WorldCommand::Open { object }
        | WorldCommand::Close { object }
        | WorldCommand::SetLocked { object, .. }
        | WorldCommand::Destroy { object } => {
            let o = tx.object_mut(content, object)?;
            require(!o.destroyed, "object destroyed")?;
            match command {
                WorldCommand::Open { .. } => {
                    require(!o.locked, "object is locked")?;
                    o.open = true;
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
