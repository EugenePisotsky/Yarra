//! Doing things that take time. A character lines up intents; one at a time is begun, takes
//! its ability's duration, then takes effect through the ability's script. Cooldowns and
//! resource costs gate what can be begun. There are no turns: everything is a point in time,
//! so the same model serves a paused-queue game and a fully real-time one.
use crate::actors::{Acting, Intent, MAX_INTENTS};
use crate::character::{change_resource, require_alive, sync_timed};
use crate::tx::Tx;
use crate::{ActScope, GameContent, GameEvent, Rejection, Result};
use game_types::*;

pub(crate) fn intend(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    intent: Intent,
    clear: bool,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let rules = &content.game.rules;
    let ability = rules.ability(&intent.ability)?;
    let character = tx.actor(actor)?;
    require_alive(content, character)?;
    if !character.abilities.contains(&intent.ability) {
        return Err(Rejection::AbilityNotKnown(intent.ability).into());
    }
    match (ability.targeted, intent.target) {
        (true, None) => return Err(Rejection::TargetRequired(intent.ability).into()),
        (false, Some(_)) => return Err(Rejection::NoTarget(intent.ability).into()),
        (true, Some(target)) => require_alive(content, tx.actor(target)?)?,
        (false, None) => {}
    }
    if !clear && character.intents.len() >= MAX_INTENTS {
        return Err(Rejection::QueueFull.into());
    }
    let character = tx.actor_mut(actor)?;
    if clear {
        character.intents.clear();
    }
    character.intents.push_back(intent);
    begin(content, tx, actor, events)?;
    sync_timed(tx, actor)
}
pub(crate) fn interrupt(tx: &mut Tx, actor: ActorId, events: &mut Vec<GameEvent>) -> Result<()> {
    let character = tx.actor_mut(actor)?;
    character.acting = None;
    character.intents.clear();
    events.push(GameEvent::Interrupted { actor });
    sync_timed(tx, actor)
}
/// Begins the next thing lined up, if the character is free and it can be begun now.
/// Intents that cannot be carried out at all are dropped on the way.
fn begin(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let rules = &content.game.rules;
    let now = tx.time;
    loop {
        let character = tx.actor(actor)?;
        if character.acting.is_some() {
            return Ok(());
        }
        let Some(intent) = character.intents.front().cloned() else {
            return Ok(());
        };
        let ability = rules.ability(&intent.ability)?;
        let obstacle = if let Some(target) = intent.target
            && !tx.actor(target)?.alive(rules)
        {
            Some(Rejection::Dead(target))
        } else {
            ability
                .costs
                .iter()
                .find(|(resource, cost)| character.resources[*resource] < **cost as i32)
                .map(|(resource, _)| Rejection::NotEnough(resource.clone()))
        };
        if let Some(reason) = obstacle {
            tx.actor_mut(actor)?.intents.pop_front();
            events.push(GameEvent::IntentDropped {
                actor,
                ability: intent.ability,
                reason,
            });
            continue;
        }
        // Not ready yet: the character waits, and time will come back to it.
        if character
            .cooldowns
            .get(&intent.ability)
            .is_some_and(|ready| *ready > now)
        {
            return Ok(());
        }
        for (resource, cost) in &ability.costs {
            change_resource(content, tx, actor, resource, -i64::from(*cost), events)?;
        }
        // Paying with the life resource can be the last thing a character does.
        if !tx.actor(actor)?.alive(rules) {
            return Ok(());
        }
        let completes_at = now.advance(ability.duration_ms)?;
        let character = tx.actor_mut(actor)?;
        character.intents.pop_front();
        events.push(GameEvent::AbilityBegun {
            actor,
            ability: intent.ability.clone(),
            target: intent.target,
            completes_at,
        });
        character.acting = Some(Acting {
            intent,
            completes_at,
        });
        return Ok(());
    }
}
/// What is due for a character's actions at the current time: what it was doing takes
/// effect, and the next thing lined up begins.
pub(crate) fn proceed(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let rules = &content.game.rules;
    let now = tx.time;
    let character = tx.actor(actor)?;
    if character.cooldowns.values().any(|ready| *ready <= now) {
        tx.actor_mut(actor)?
            .cooldowns
            .retain(|_, ready| *ready > now);
    }
    let due = tx
        .actor(actor)?
        .acting
        .as_ref()
        .is_some_and(|acting| acting.completes_at <= now);
    if due {
        let Acting { intent, .. } = tx.actor_mut(actor)?.acting.take().expect("checked");
        let ability = rules.ability(&intent.ability)?;
        let target = intent.target;
        if let Some(dead) = target.filter(|t| tx.actors.get(t).is_none_or(|a| !a.alive(rules))) {
            events.push(GameEvent::IntentDropped {
                actor,
                ability: intent.ability.clone(),
                reason: Rejection::Dead(dead),
            });
        } else {
            // In the script the user is the scene's player and the target its speaker.
            // What it does takes effect as a whole or not at all, and a script that fails
            // costs its user the action, not the rest of the queue, and does not stop time
            // for everyone.
            let savepoint = tx.savepoint();
            let mark = events.len();
            let outcome = content.scripts.engine(&ability.resolve).and_then(|engine| {
                engine.action(
                    &ability.resolve,
                    &mut ActScope {
                        content,
                        tx,
                        player: actor,
                        speaker: target.unwrap_or(actor),
                        events,
                    },
                )
            });
            if let Err(error) = outcome {
                tx.rollback_to(savepoint);
                events.truncate(mark);
                events.push(GameEvent::AbilityFailed {
                    actor,
                    ability: intent.ability.clone(),
                    reason: error.to_string(),
                });
            } else {
                tx.release(savepoint);
                if ability.cooldown_ms > 0 {
                    let ready = now.advance(ability.cooldown_ms)?;
                    tx.actor_mut(actor)?
                        .cooldowns
                        .insert(intent.ability.clone(), ready);
                }
                events.push(GameEvent::AbilityResolved {
                    actor,
                    ability: intent.ability.clone(),
                    target,
                });
                let alive = |id: ActorId| tx.actors.get(&id).is_some_and(|a| a.alive(rules));
                if intent.repeat && alive(actor) && target.is_none_or(alive) {
                    let character = tx.actor_mut(actor)?;
                    if character.intents.len() < MAX_INTENTS {
                        character.intents.push_back(intent);
                    }
                }
            }
        }
    }
    if tx.actor(actor)?.alive(rules) {
        begin(content, tx, actor, events)?;
    }
    Ok(())
}
