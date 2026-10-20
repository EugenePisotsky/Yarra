//! Reusable predicates and NPC entry profiles. All evaluation uses an already-resolved snapshot.
use crate::{Condition, GameContent, Rejection, Result, SessionState, quests};
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueVariant {
    pub dialogue: DialogueId,
    pub weight: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryRule {
    pub id: Key,
    pub priority: i32,
    /// Explicit tie order within a priority tier. Duplicate (priority, order) pairs are errors.
    pub order: u16,
    /// Topics are independently discoverable; opening selection considers non-topic rules.
    pub topic: Option<TextRef>,
    pub condition: Condition,
    pub variants: Vec<DialogueVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProfile {
    pub id: InteractionProfileId,
    pub rules: Vec<EntryRule>,
}

impl InteractionProfile {
    pub fn validate(&self) -> Result<()> {
        require(!self.rules.is_empty(), "a profile has entry rules")?;
        let mut ids = BTreeSet::new();
        let mut precedence = BTreeSet::new();

        for rule in &self.rules {
            require(
                ids.insert(&rule.id) && precedence.insert((rule.priority, rule.order)),
                "duplicate rule or ambiguous interaction precedence",
            )?;

            if let Some(text) = &rule.topic {
                text.validate()?;
            }

            require(!rule.variants.is_empty(), "an entry rule has variants")?;
            let mut dialogues = BTreeSet::new();
            let mut weight = 0u32;

            for variant in &rule.variants {
                require(
                    variant.weight > 0 && dialogues.insert(variant.dialogue),
                    "duplicate or zero-weight variant",
                )?;
                weight = weight
                    .checked_add(variant.weight)
                    .ok_or_else(|| Invalid("variant weight overflow".into()))?;
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPreview {
    pub rule: Key,
    pub priority: i32,
    pub order: u16,
    pub topic: Option<TextRef>,
    /// Whether the rule's condition holds and a variant is available.
    pub matched: bool,
    /// Variants excluded by their scoped repeat/cooldown policy.
    pub unavailable_variants: Vec<DialogueId>,
    pub variants: Vec<DialogueVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionPreview {
    pub profile: InteractionProfileId,
    pub candidates: Vec<EntryPreview>,
}

impl InteractionPreview {
    pub fn opening(&self) -> Option<&EntryPreview> {
        self.candidates
            .iter()
            .find(|r| r.topic.is_none() && r.matched)
    }

    pub fn topics(&self) -> impl Iterator<Item = &EntryPreview> {
        self.candidates
            .iter()
            .filter(|r| r.topic.is_some() && r.matched)
    }

    pub(crate) fn selected(&self, topic: Option<&Key>) -> Result<&EntryPreview> {
        match topic {
            None => self.opening(),
            Some(id) => self.topics().find(|r| &r.rule == id),
        }
        .ok_or_else(|| Invalid("no eligible conversation entry".into()).into())
    }
}

impl GameContent {
    pub fn quest(&self, id: QuestId) -> Result<&quests::Quest> {
        self.game
            .quests
            .get(&id)
            .ok_or_else(|| Invalid(format!("unknown quest {id}")).into())
    }

    pub fn profile(&self, id: InteractionProfileId) -> Result<&InteractionProfile> {
        self.game
            .profiles
            .get(&id)
            .ok_or_else(|| Invalid(format!("unknown interaction profile {id}")).into())
    }

    /// The profile that says what `speaker` has to say to `participant`.
    pub(crate) fn interaction(
        &self,
        state: &SessionState,
        participant: ActorId,
        speaker: ActorId,
    ) -> Result<InteractionProfileId> {
        require(participant != speaker, "interaction needs two actors")?;
        state.actor(participant)?;
        let template = self.template(state.actor(speaker)?.template)?;
        template
            .interaction
            .ok_or_else(|| Rejection::NothingToSay(speaker).into())
    }

    pub fn preview(
        &self,
        profile: InteractionProfileId,
        state: &SessionState,
        player: ActorId,
        speaker: ActorId,
    ) -> Result<InteractionPreview> {
        let mut candidates = Vec::new();

        for rule in &self.profile(profile)?.rules {
            let mut variants = Vec::new();
            let mut unavailable_variants = Vec::new();

            for variant in &rule.variants {
                let contract = self.dialogue_contract(variant.dialogue)?;
                if contract.repeat.eligible(
                    &state.history(contract.history_key(player, speaker)),
                    state.time,
                ) {
                    variants.push(variant.clone());
                } else {
                    unavailable_variants.push(variant.dialogue);
                }
            }

            let holds = self.evaluate(&rule.condition, state, player, speaker)?;
            candidates.push(EntryPreview {
                rule: rule.id.clone(),
                priority: rule.priority,
                order: rule.order,
                topic: rule.topic.clone(),
                matched: holds && !variants.is_empty(),
                variants,
                unavailable_variants,
            });
        }

        candidates.sort_by_key(|c| (std::cmp::Reverse(c.priority), c.order));
        Ok(InteractionPreview {
            profile,
            candidates,
        })
    }
}
