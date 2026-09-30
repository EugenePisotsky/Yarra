use crate::Result;
use crate::resolution::{resolve, resolve_with};
use crate::*;
use game_types::*;
use std::collections::BTreeSet;

/// All durable notifications are derived from accepted changes, including dialogue actions.
pub(crate) fn stage<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    source: &mut C,
    before: &WorkingSet,
    after: &SessionState,
) -> Result<()> {
    let old = &before.state;
    let mut signals = BTreeSet::new();
    for a in &after.actors {
        let quantities = |bag: &inventory::Inventory| {
            let mut totals = std::collections::BTreeMap::new();
            for entry in &bag.entries {
                *totals.entry(entry.definition).or_insert(0u64) += u64::from(entry.quantity);
            }
            totals
        };
        let before_items = quantities(old.carried(a.id)?);
        for (definition, count) in quantities(after.carried(a.id)?) {
            if count > before_items.get(&definition).copied().unwrap_or(0) {
                signals.insert(WorldSignal::ItemAcquired {
                    actor: a.id,
                    definition,
                });
            }
        }
        if old.actor(a.id)? != a {
            signals.insert(WorldSignal::Actor(a.id));
        }
    }
    for i in &after.inventories {
        if old.inventory(i.id)? != i && i.owner.kind == "actor" {
            signals.insert(WorldSignal::Actor(ActorId(i.owner.id.0)));
        }
    }
    for q in &after.quests {
        if old.quest(q.quest)? != q {
            signals.insert(WorldSignal::Quest(q.quest));
            if old.quest(q.quest)?.status == quests::Status::NotStarted
                && q.status == quests::Status::Active
            {
                signals.insert(WorldSignal::QuestStarted(q.quest));
            }
        }
    }
    for h in &after.histories {
        if old.history(h.key)? != h {
            signals.insert(WorldSignal::History(h.key));
        }
    }
    for c in &after.claims {
        if old.claim(c.key)? != c {
            signals.insert(WorldSignal::Claim(c.key));
        }
    }
    for r in &after.relationships {
        if old.relationship(r.key)? != r {
            signals.insert(WorldSignal::Relationship(r.key));
        }
    }
    for key in old.facts.symmetric_difference(&after.facts) {
        signals.insert(WorldSignal::Fact(key.clone()));
    }
    for l in &after.world.locations {
        let previous = old.world.location(l.actor)?;
        for area in l.areas.difference(&previous.areas) {
            signals.insert(WorldSignal::Entered {
                actor: l.actor,
                area: *area,
            });
        }
        for area in previous.areas.difference(&l.areas) {
            signals.insert(WorldSignal::Exited {
                actor: l.actor,
                area: *area,
            });
        }
    }
    let mut subscribed = BTreeSet::new();
    for signal in signals {
        if source.next_trigger(&signal, None)?.is_some() {
            subscribed.insert(signal);
        }
    }
    tx.stage(before, after)?;
    tx.enqueue(&subscribed)
}
fn save<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    source: &mut C,
    content: &GameContent,
    before: &WorkingSet,
    next: &mut SessionState,
    header: &mut SessionHeader,
    events: &mut [GameEvent],
) -> Result<()> {
    crate::session::assign_item_ids(&before.state, next, events);
    next.validate(content)?;
    stage(tx, source, before, next)?;
    header.random = next.random;
    Ok(())
}
fn position<C: ContentSource>(
    source: &mut C,
    state: &mut SessionState,
    actor: ActorId,
    p: actors::Position,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let areas = source.areas_at(&p)?;
    let loc = state
        .world
        .locations
        .iter_mut()
        .find(|l| l.actor == actor)
        .ok_or_else(|| Invalid("location not resolved".into()))?;
    for area in areas.difference(&loc.areas) {
        events.push(GameEvent::World(WorldEvent::Entered { actor, area: *area }));
    }
    for area in loc.areas.difference(&areas) {
        events.push(GameEvent::World(WorldEvent::Exited { actor, area: *area }));
    }
    loc.areas = areas;
    state.actor_mut(actor)?.position = p;
    Ok(())
}
fn sequence<T: StateTransaction>(
    tx: &mut T,
    content: &GameContent,
    state: &mut SessionState,
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
                        state,
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
                    tx.movement_owner(actor)?
                        .is_none_or(|owner| owner == progress.id),
                    "actor movement is owned by another sequence",
                )?;
                require(
                    state.actor(actor)?.health > 0,
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
                    deadline: state.time.advance(*timeout_ms)?,
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
    let trigger_id = progress.id;
    *state
        .world
        .triggers
        .iter_mut()
        .find(|t| t.id == trigger_id)
        .ok_or_else(|| Invalid("trigger not resolved".into()))? = progress;
    Ok(())
}

/// Failed immediate plans publish a bounded diagnostic, without consuming claims or partial effects.
/// Storage failures still reject the whole command; the delivery remains pending for explicit retry.
fn run_sequence<T: StateTransaction>(
    tx: &mut T,
    content: &GameContent,
    state: &mut SessionState,
    mut progress: TriggerState,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let before = state.clone();
    let event_count = events.len();
    match sequence(tx, content, state, progress.clone(), events) {
        Ok(()) => Ok(()),
        Err(error @ GameplayError::Runtime(_)) => Err(error),
        Err(error) => {
            *state = before;
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
            let id = progress.id;
            *state
                .world
                .triggers
                .iter_mut()
                .find(|t| t.id == id)
                .unwrap() = progress;
            Ok(())
        }
    }
}

fn trigger_context<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    source: &mut C,
    id: TriggerId,
) -> Result<(WorkingSet, GameContent)> {
    resolve(
        tx,
        source,
        &StateRequest {
            triggers: [id].into(),
            ..Default::default()
        },
    )
}
fn complete<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    source: &mut C,
    id: ActionId,
    result: MoveResult,
    header: &mut SessionHeader,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let (batch, mut content) = trigger_context(tx, source, id.trigger)?;
    let mut next = batch.state.clone();
    let mut progress = next.world.trigger(id.trigger)?.clone();
    if let Some((_, receipt)) = progress.receipts.iter().find(|(a, _)| *a == id) {
        require(receipt == &result, "conflicting movement result")?;
        return Ok(());
    }
    let pending = progress
        .movement
        .as_ref()
        .ok_or_else(|| Invalid("movement is not pending".into()))?;
    require(pending.id == id, "stale movement result")?;
    if !matches!(result, MoveResult::TimedOut) {
        require(
            header.time < pending.deadline,
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
                next.actor(pending.actor)?.health > 0,
                "dead actor cannot arrive",
            )?;
            let ids = source.areas_at(p)?;
            if !ids.is_empty() {
                let (_, extended) = resolve_with(
                    tx,
                    source,
                    &StateRequest {
                        triggers: [id.trigger].into(),
                        ..Default::default()
                    },
                    &ContentRequest {
                        areas: ids,
                        ..Default::default()
                    },
                )?;
                content = extended;
            }
            position(source, &mut next, pending.actor, p.clone(), events)?;
            progress.step += 1;
        }
        MoveResult::Failed { reason } => {
            progress.status = SequenceStatus::Failed {
                reason: reason.clone(),
            }
        }
        MoveResult::Cancelled => progress.status = SequenceStatus::Cancelled,
        MoveResult::TimedOut => {
            require(
                header.time >= pending.deadline,
                "movement has not timed out",
            )?;
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
        run_sequence(tx, &content, &mut next, progress, events)?;
    } else {
        events.push(GameEvent::World(WorldEvent::SequenceChanged {
            trigger: progress.id,
            status: progress.status.clone(),
        }));
        *next
            .world
            .triggers
            .iter_mut()
            .find(|t| t.id == id.trigger)
            .unwrap() = progress;
    }
    save(tx, source, &content, &batch, &mut next, header, events)
}
pub(crate) fn apply<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    source: &mut C,
    command: WorldCommand,
    header: &mut SessionHeader,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    match command {
        WorldCommand::ProcessNext => {
            if let Some(id) = tx.next_movement(None, Some(header.time))? {
                let (batch, _) = trigger_context(tx, source, id)?;
                let action = batch
                    .state
                    .world
                    .trigger(id)?
                    .movement
                    .as_ref()
                    .ok_or_else(|| Invalid("movement index mismatch".into()))?
                    .id;
                return complete(tx, source, action, MoveResult::TimedOut, header, events);
            }
            let Some(event) = tx.next_event()? else {
                return Ok(());
            };
            let candidate = source.next_trigger(&event.signal, event.after)?;
            if let Some(id) = candidate {
                let (batch, content) = trigger_context(tx, source, id)?;
                let d = content.trigger(id)?;
                require(
                    d.subscriptions(&content)?.contains(&event.signal),
                    "trigger subscription index mismatch",
                )?;
                let mut next = batch.state.clone();
                let mut progress = next.world.trigger(id)?.clone();
                if progress.eligible(d, next.time)
                    && content
                        .evaluate(&d.condition, &next, d.participant, d.speaker)?
                        .matched
                {
                    progress.run = progress
                        .run
                        .checked_add(1)
                        .ok_or_else(|| Invalid("sequence run overflow".into()))?;
                    progress.step = 0;
                    progress.status = SequenceStatus::Running;
                    progress.last_started = Some(next.time);
                    progress.receipts.clear();
                    progress.diagnostic = None;
                    run_sequence(tx, &content, &mut next, progress, events)?;
                    save(tx, source, &content, &batch, &mut next, header, events)?;
                }
            }
            tx.advance_event(&event, candidate)?;
            events.push(GameEvent::World(WorldEvent::DeliveryProcessed(event.id)));
        }
        WorldCommand::RetryTrigger { trigger } => {
            let (batch, content) = trigger_context(tx, source, trigger)?;
            let d = content.trigger(trigger)?;
            let mut next = batch.state.clone();
            let mut progress = next.world.trigger(trigger)?.clone();
            require(
                matches!(
                    progress.status,
                    SequenceStatus::Failed { .. } | SequenceStatus::Cancelled
                ),
                "only failed or cancelled sequences can retry",
            )?;
            progress.consecutive_failures = 0;
            require(
                progress.eligible(d, next.time)
                    && content
                        .evaluate(&d.condition, &next, d.participant, d.speaker)?
                        .matched,
                "trigger retry is ineligible",
            )?;
            progress.run = progress
                .run
                .checked_add(1)
                .ok_or_else(|| Invalid("sequence run overflow".into()))?;
            progress.step = 0;
            progress.status = SequenceStatus::Running;
            progress.last_started = Some(next.time);
            progress.receipts.clear();
            progress.diagnostic = None;
            run_sequence(tx, &content, &mut next, progress, events)?;
            save(tx, source, &content, &batch, &mut next, header, events)?;
        }
        WorldCommand::ObservePosition {
            actor,
            observation,
            position: p,
        } => {
            let ids = source.areas_at(&p)?;
            let (batch, content) = resolve_with(
                tx,
                source,
                &StateRequest {
                    actors: [actor].into(),
                    locations: [actor].into(),
                    ..Default::default()
                },
                &ContentRequest {
                    areas: ids,
                    ..Default::default()
                },
            )?;
            let mut next = batch.state.clone();
            let loc = next
                .world
                .locations
                .iter_mut()
                .find(|l| l.actor == actor)
                .unwrap();
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
            loc.observation = observation;
            loc.last_observed = Some(p.clone());
            position(source, &mut next, actor, p, events)?;
            save(tx, source, &content, &batch, &mut next, header, events)?;
        }
        WorldCommand::FinishMove { id, result } => {
            complete(tx, source, id, result, header, events)?
        }
        WorldCommand::StartMove { id } => {
            let (batch, content) = trigger_context(tx, source, id.trigger)?;
            let mut next = batch.state.clone();
            let p = next
                .world
                .triggers
                .iter_mut()
                .find(|t| t.id == id.trigger)
                .unwrap();
            if p.receipts.iter().any(|(a, _)| *a == id) {
                return Ok(());
            }
            let m = p
                .movement
                .as_mut()
                .ok_or_else(|| Invalid("movement not pending".into()))?;
            require(
                m.id == id && header.time < m.deadline,
                "stale movement start",
            )?;
            m.phase = MovementPhase::Running;
            save(tx, source, &content, &batch, &mut next, header, events)?;
        }
        WorldCommand::Open { object }
        | WorldCommand::Close { object }
        | WorldCommand::SetLocked { object, .. }
        | WorldCommand::Destroy { object } => {
            let (batch, content) = resolve(
                tx,
                source,
                &StateRequest {
                    objects: [object].into(),
                    ..Default::default()
                },
            )?;
            let mut next = batch.state.clone();
            let o = next
                .world
                .objects
                .iter_mut()
                .find(|o| o.id == object)
                .unwrap();
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
            save(tx, source, &content, &batch, &mut next, header, events)?;
        }
    }
    Ok(())
}
