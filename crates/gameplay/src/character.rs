//! What happens to characters: stats are worked out, resources change, effects come and go,
//! levels are gained and points spent. Every function works on the command's journal, so a
//! command that fails later leaves no trace.
use crate::actors::Actor;
use crate::inventory::Money;
use crate::rules::{ActiveEffect, Rules, Sheet};
use crate::tx::Tx;
use crate::{GameContent, GameEvent, Rejection, Result};
use game_types::*;

/// Works the character's stats out again and keeps its resources within their caps.
/// A resource the character did not have yet starts full.
pub(crate) fn refresh(content: &GameContent, tx: &mut Tx, actor: ActorId) -> Result<()> {
    let stats = tx.sheet(content, tx.actor(actor)?)?;
    tx.actor_mut(actor)?.set_stats(stats, &content.game.rules);
    Ok(())
}
/// Keeps the list of characters with something pending in step with this character.
pub(crate) fn sync_timed(tx: &mut Tx, actor: ActorId) -> Result<()> {
    let timed = tx.actor(actor)?.next_event().is_some();
    if tx.timed.contains(&actor) != timed {
        if timed {
            tx.timed_mut().insert(actor);
        } else {
            tx.timed_mut().remove(&actor);
        }
    }
    Ok(())
}
/// The actor's carried inventory, made now if nothing needed it before: what its template
/// wears, equipped, and what its loot table yields.
pub(crate) fn carried(content: &GameContent, tx: &mut Tx, actor: ActorId) -> Result<InventoryId> {
    if let Some(id) = tx.carried.get(&actor) {
        return Ok(*id);
    }
    let (inventory, equipment) = tx.unopened_inventory(content, tx.actor(actor)?)?;
    let id = inventory.id;
    tx.put_inventory(inventory);
    if !equipment.is_empty() {
        tx.actor_mut(actor)?.equipment = equipment;
    }
    Ok(id)
}
/// Makes a container's contents the first time it is opened.
pub(crate) fn fill_container(content: &GameContent, tx: &mut Tx, object: ObjectId) -> Result<()> {
    if let crate::ObjectKind::Container { inventory, .. } = content.object(object)?.kind
        && !tx.inventories.contains_key(&inventory)
    {
        let contents = tx.unopened_container(content, object)?;
        tx.put_inventory(contents);
    }
    Ok(())
}
pub(crate) fn require_alive(content: &GameContent, actor: &Actor) -> Result<()> {
    if actor.alive(&content.game.rules) {
        Ok(())
    } else {
        Err(Rejection::Dead(actor.id).into())
    }
}
/// Adds to a resource, or takes from it, within zero and its cap. Losing the last of the
/// life resource is death.
pub(crate) fn change_resource(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    resource: &Key,
    amount: i64,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let rules = &content.game.rules;
    let maximum = rules.resource(resource)?;
    let character = tx.actor(actor)?;
    let (old, cap) = (character.resources[resource], character.stats[maximum]);
    let now = (i64::from(old) + amount).clamp(0, i64::from(cap.max(0))) as i32;
    if now == old {
        return Ok(());
    }
    tx.actor_mut(actor)?.resources.insert(resource.clone(), now);
    events.push(GameEvent::ResourceChanged {
        actor,
        resource: resource.clone(),
        change: now - old,
        now,
    });
    if resource == &rules.life && now == 0 {
        // Nothing lingers on the dead, and the dead do nothing.
        let character = tx.actor_mut(actor)?;
        character.effects.clear();
        character.acting = None;
        character.intents.clear();
        refresh(content, tx, actor)?;
        sync_timed(tx, actor)?;
        events.push(GameEvent::Died { actor });
    }
    Ok(())
}
/// Puts an effect on a character. One it already has starts over.
pub(crate) fn apply_effect(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    effect: &Key,
    duration_ms: Option<u64>,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let definition = content.game.rules.effect(effect)?;
    require_alive(content, tx.actor(actor)?)?;
    let now = tx.time;
    let active = ActiveEffect {
        effect: effect.clone(),
        expires_at: duration_ms.map(|ms| now.advance(ms)).transpose()?,
        next_tick: definition
            .periodic
            .as_ref()
            .map(|p| now.advance(p.period_ms))
            .transpose()?,
    };
    let character = tx.actor_mut(actor)?;
    character.effects.retain(|e| &e.effect != effect);
    character.effects.push(active);
    // Kept in one order, so two characters with the same effects compare equal.
    character.effects.sort_by(|a, b| a.effect.cmp(&b.effect));
    refresh(content, tx, actor)?;
    sync_timed(tx, actor)?;
    events.push(GameEvent::EffectApplied {
        actor,
        effect: effect.clone(),
    });
    Ok(())
}
/// Takes an effect off a character, if it is there.
pub(crate) fn remove_effect(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    effect: &Key,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    content.game.rules.effect(effect)?;
    if !tx.actor(actor)?.effects.iter().any(|e| &e.effect == effect) {
        return Ok(());
    }
    tx.actor_mut(actor)?.effects.retain(|e| &e.effect != effect);
    refresh(content, tx, actor)?;
    sync_timed(tx, actor)?;
    events.push(GameEvent::EffectEnded {
        actor,
        effect: effect.clone(),
    });
    Ok(())
}
/// Moves the clock, carrying out in order of time whatever falls due on the way: effects
/// tick and end, actions take effect and the next ones begin. Only characters with
/// something pending are looked at.
pub(crate) fn advance_time(
    content: &GameContent,
    tx: &mut Tx,
    millis: u64,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let target = tx.time.advance(millis)?;
    require(
        target.0 <= i64::MAX as u64,
        "logical time exceeds storage range",
    )?;
    // Everything due takes time to happen again, so this ends.
    loop {
        let due = tx
            .timed
            .iter()
            .filter_map(|id| Some((tx.actors.get(id)?.next_event()?, *id)))
            .filter(|(time, _)| *time <= target)
            .min();
        let Some((time, actor)) = due else {
            tx.set_time(target);
            events.push(GameEvent::TimeAdvanced);
            return Ok(());
        };
        tx.set_time(time);
        happen(content, tx, actor, events)?;
    }
}
/// Carries out what is due for one character at the current time.
fn happen(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let now = tx.time;
    let rules = &content.game.rules;
    // An effect that ticks at the moment it ends still ticks.
    let ticking: Vec<Key> = tx
        .actor(actor)?
        .effects
        .iter()
        .filter(|e| e.next_tick.is_some_and(|t| t <= now))
        .map(|e| e.effect.clone())
        .collect();
    for effect in ticking {
        let Some(periodic) = &rules.effect(&effect)?.periodic else {
            continue;
        };
        // Death clears the effects, the rest of this list included.
        let Some(active) = tx
            .actor_mut(actor)?
            .effects
            .iter_mut()
            .find(|e| e.effect == effect)
        else {
            break;
        };
        active.next_tick = Some(now.advance(periodic.period_ms)?);
        change_resource(
            content,
            tx,
            actor,
            &periodic.resource,
            i64::from(periodic.amount),
            events,
        )?;
    }
    let ended: Vec<Key> = tx
        .actor(actor)?
        .effects
        .iter()
        .filter(|e| e.expires_at.is_some_and(|t| t <= now))
        .map(|e| e.effect.clone())
        .collect();
    for effect in ended {
        remove_effect(content, tx, actor, &effect, events)?;
    }
    crate::combat::proceed(content, tx, actor, events)?;
    sync_timed(tx, actor)
}
/// Adds to the party's shared experience; every member gains the levels it has earned.
pub(crate) fn award_experience(
    content: &GameContent,
    tx: &mut Tx,
    amount: u64,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let party = tx.party_mut();
    party.experience = party
        .experience
        .checked_add(amount)
        .filter(|total| *total <= i64::MAX as u64)
        .ok_or_else(|| Invalid("experience overflow".into()))?;
    events.push(GameEvent::ExperienceAwarded {
        amount,
        total: party.experience,
    });
    for member in tx.party.members.clone() {
        catch_up(content, tx, member, events)?;
    }
    Ok(())
}
/// Brings a party member to the level the party's experience has earned.
pub(crate) fn catch_up(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let rules = &content.game.rules;
    let earned = rules.level_for(tx.party.experience);
    if tx.actor(actor)?.level >= earned {
        return Ok(());
    }
    while tx.actor(actor)?.level < earned {
        let character = tx.actor_mut(actor)?;
        character.gain_level(rules)?;
        events.push(GameEvent::LeveledUp {
            actor,
            level: character.level,
        });
    }
    refresh(content, tx, actor)
}
fn member(tx: &Tx, actor: ActorId) -> Result<()> {
    if tx.party.members.contains(&actor) {
        Ok(())
    } else {
        Err(Rejection::NotInParty(actor).into())
    }
}
pub(crate) fn spend_attribute_point(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    stat: &Key,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    member(tx, actor)?;
    let (_, maximum) = content.game.rules.primary(stat)?;
    let character = tx.actor(actor)?;
    if character.attribute_points == 0 {
        return Err(Rejection::NoAttributePoints.into());
    }
    if character.base[stat] >= maximum {
        return Err(Rejection::StatAtMaximum(stat.clone()).into());
    }
    let character = tx.actor_mut(actor)?;
    character.attribute_points -= 1;
    *character.base.get_mut(stat).expect("checked primary stat") += 1;
    refresh(content, tx, actor)?;
    events.push(GameEvent::StatRaised {
        actor,
        stat: stat.clone(),
    });
    Ok(())
}
/// The rank a trainer could teach this character next and the learning points it costs,
/// or why not.
pub fn teachable(
    rules: &Rules,
    actor: &Actor,
    skill: &Key,
) -> Result<std::result::Result<(u8, u32), Rejection>> {
    let definition = rules.skill(skill)?;
    let class = rules.class(&actor.class)?;
    let rank = actor.skill(skill);
    Ok(if usize::from(rank) >= definition.ranks.len() {
        Err(Rejection::SkillAtMaximum(skill.clone()))
    } else if rank >= class.skills.get(skill).copied().unwrap_or(0) {
        Err(Rejection::ClassCannotLearn {
            class: actor.class.clone(),
            skill: skill.clone(),
        })
    } else {
        let needed = definition.ranks[usize::from(rank)];
        if actor.learning_points < needed {
            Err(Rejection::NotEnoughLearningPoints {
                needed,
                available: actor.learning_points,
            })
        } else {
            Ok((rank + 1, needed))
        }
    })
}
pub(crate) fn teach(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    skill: &Key,
    events: &mut Vec<GameEvent>,
) -> Result<()> {
    let (rank, cost) = teachable(&content.game.rules, tx.actor(actor)?, skill)??;
    let character = tx.actor_mut(actor)?;
    character.learning_points -= cost;
    character.skills.insert(skill.clone(), rank);
    refresh(content, tx, actor)?;
    events.push(GameEvent::SkillLearned {
        actor,
        skill: skill.clone(),
        rank,
    });
    Ok(())
}
/// Takes gold from the party's shared purse.
pub(crate) fn pay(tx: &mut Tx, amount: u64, events: &mut Vec<GameEvent>) -> Result<()> {
    let wallet = tx
        .party
        .wallet
        .ok_or_else(|| Invalid("the party has no purse".into()))?;
    let amount = Money::new(amount)?;
    if tx.wallet(wallet)?.balance < amount {
        return Err(Rejection::NotEnoughGold {
            needed: amount.units(),
            available: tx.wallet(wallet)?.balance.units(),
        }
        .into());
    }
    tx.wallet_mut(wallet)?.debit(amount)?;
    events.push(GameEvent::Paid {
        amount: amount.units(),
    });
    Ok(())
}
/// Asks the rules' formula whether a check passes. Whatever it rolls comes from the saved
/// random stream and is reported with the outcome.
pub(crate) fn check(
    content: &GameContent,
    tx: &mut Tx,
    actor: ActorId,
    skill: &Key,
    difficulty: u32,
    events: &mut Vec<GameEvent>,
) -> Result<bool> {
    let rules = &content.game.rules;
    rules.skill(skill)?;
    let engine = content.scripts.engine(&rules.check)?;
    let character = tx.actor(actor)?;
    let mut random = tx.random;
    let mut rolls = Vec::new();
    let passed = engine.check(
        &rules.check,
        &Sheet {
            level: character.level,
            class: &character.class,
            stats: &character.stats,
            skills: &character.skills,
        },
        skill,
        difficulty,
        &mut |sides| {
            let roll = random.die(sides)?;
            rolls.push(roll);
            Ok(roll)
        },
    )?;
    *tx.random_mut() = random;
    events.push(GameEvent::Checked {
        actor,
        skill: skill.clone(),
        passed,
        rolls,
    });
    Ok(passed)
}
