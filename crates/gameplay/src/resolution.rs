use crate::Result;
use crate::*;
use std::collections::BTreeSet;

pub(crate) fn resolve<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    content: &mut C,
    request: &StateRequest,
) -> Result<(WorkingSet, GameContent)> {
    resolve_with(tx, content, request, &ContentRequest::default())
}
pub(crate) fn resolve_with<T: StateTransaction, C: ContentSource>(
    tx: &mut T,
    content: &mut C,
    request: &StateRequest,
    extra: &ContentRequest,
) -> Result<(WorkingSet, GameContent)> {
    let mut batch = tx.load(request)?;
    let mut needed = ContentRequest::for_state(&batch.state, request);
    needed.objects.extend(&extra.objects);
    needed.areas.extend(&extra.areas);
    needed.triggers.extend(&extra.triggers);
    needed.dialogue_contracts.extend(&extra.dialogue_contracts);
    needed.claims.extend(&extra.claims);
    needed.items.extend(&extra.items);
    needed.actors.extend(&extra.actors);
    needed.dialogues.extend(&extra.dialogues);
    needed.facts.extend(extra.facts.clone());
    needed.quests.extend(&extra.quests);
    needed.profiles.extend(&extra.profiles);
    needed.predicates.extend(&extra.predicates);
    let resolved = content.resolve(&needed)?;
    let mut expanded = request.clone();
    expanded
        .quests
        .extend(resolved.game.quests.iter().map(|q| q.id));
    let contexts: BTreeSet<_> = request
        .conversations
        .iter()
        .map(|c| (c.participant, c.speaker))
        .chain(
            request
                .interactions
                .iter()
                .map(|i| (i.participant, i.speaker)),
        )
        .chain(
            resolved
                .game
                .world
                .triggers
                .iter()
                .map(|t| (t.participant, t.speaker)),
        )
        .collect();
    expanded
        .objects
        .extend(resolved.game.world.objects.iter().map(|o| o.id));
    for (p, s) in &contexts {
        expanded.actors.extend([*p, *s]);
        if !resolved.game.world.areas.is_empty() || !resolved.game.world.triggers.is_empty() {
            expanded.locations.extend([*p, *s]);
        }
    }
    for (p, s) in &contexts {
        for contract in &resolved.game.dialogue_contracts {
            expanded.histories.insert(contract.history_key(*p, *s));
        }
        for claim in &resolved.game.claims {
            expanded.claims.insert(claim.key(*p, *s));
        }
    }
    let mut pairs = BTreeSet::new();
    let mut relationship = |from: Participant, to: Participant| {
        for (p, s) in &contexts {
            pairs.insert(actors::RelationshipKey {
                from: from.resolve(*p, *s),
                to: to.resolve(*p, *s),
            });
        }
    };
    for c in resolved
        .game
        .conditions
        .values()
        .chain(resolved.game.predicates.iter().map(|p| &p.condition))
        .chain(resolved.game.world.triggers.iter().map(|t| &t.condition))
        .chain(
            resolved
                .game
                .profiles
                .iter()
                .flat_map(|p| p.rules.iter().map(|r| &r.condition)),
        )
    {
        c.visit(&mut |c| {
            if let Condition::Relationship { from, to, .. } = c {
                relationship(*from, *to);
            }
            Ok(())
        })?;
    }
    for a in resolved.game.actions.values().chain(
        resolved
            .game
            .world
            .triggers
            .iter()
            .flat_map(|t| t.steps.iter())
            .filter_map(|s| {
                if let SequenceStep::Apply(a) = s {
                    Some(a)
                } else {
                    None
                }
            })
            .flatten(),
    ) {
        a.visit(&mut |a| {
            if let Action::Relationship { from, to, .. } = a {
                relationship(*from, *to);
            }
            Ok(())
        })?;
    }
    expanded.relationships.extend(pairs);
    if expanded != *request {
        batch = tx.load(&expanded)?;
    }
    for loc in &mut batch.state.world.locations {
        if batch.versions.get(&RecordKey::Location(loc.actor)) == Some(&0) {
            let actor = batch
                .state
                .actors
                .iter()
                .find(|a| a.id == loc.actor)
                .ok_or_else(|| game_types::Invalid("location actor missing".into()))?;
            loc.areas = content.areas_at(&actor.position)?;
        }
    }
    let first_request = needed.clone();
    let mut complete = ContentRequest::for_state(&batch.state, &expanded);
    complete.objects.extend(needed.objects);
    complete.areas.extend(needed.areas);
    complete.triggers.extend(needed.triggers);
    complete.items.extend(needed.items);
    complete.actors.extend(needed.actors);
    complete.dialogues.extend(needed.dialogues);
    complete.profiles.extend(needed.profiles);
    complete.predicates.extend(needed.predicates);
    complete
        .dialogue_contracts
        .extend(needed.dialogue_contracts);
    complete.claims.extend(needed.claims);
    complete.quests.extend(needed.quests);
    complete.facts.extend(needed.facts);
    let resolved = if complete == first_request {
        resolved
    } else {
        content.resolve(&complete)?
    };
    for object in &mut batch.state.world.objects {
        if batch.versions.get(&RecordKey::Object(object.id)) == Some(&0) {
            *object = ObjectState::initial(resolved.object(object.id)?);
        }
    }
    batch.state.facts = tx.facts(&resolved.game.facts)?;
    batch.state.validate(&resolved)?;
    Ok((batch, resolved))
}
