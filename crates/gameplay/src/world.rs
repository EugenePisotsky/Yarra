//! The logical world: objects and their locks, which areas actors are in, movement the
//! engine carries out, and triggers that react to what happens. Shapes and positions belong
//! to the engine; gameplay knows areas only by name and is told who is inside them.
use crate::Result;
use crate::actors::Position;
use crate::*;
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldDefinitions {
    #[serde(with = "crate::keyed::list")]
    pub objects: BTreeMap<ObjectId, ObjectDefinition>,
    /// Areas this content refers to. The world supplies their shapes under the same names.
    pub areas: BTreeSet<AreaId>,
    #[serde(with = "crate::keyed::list")]
    pub triggers: BTreeMap<TriggerId, TriggerDefinition>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectKind {
    Door,
    Container {
        inventory: InventoryId,
        /// What is inside the first time it is opened.
        #[serde(default)]
        loot: Option<LootId>,
    },
}
/// Stable placed-object identity, independent of whether its region is currently loaded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectDefinition {
    pub id: ObjectId,
    pub name: TextRef,
    pub kind: ObjectKind,
    pub locked: bool,
}
/// Something that happened, which triggers can listen for.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WorldSignal {
    Entered {
        actor: ActorId,
        area: AreaId,
    },
    Exited {
        actor: ActorId,
        area: AreaId,
    },
    /// A movement requested with `Move` reached its area.
    Arrived {
        actor: ActorId,
        area: AreaId,
    },
    /// A requested movement was given up or ran out of time.
    MoveFailed(ActorId),
    Died(ActorId),
    /// The character gained at least one level.
    LeveledUp(ActorId),
    /// The actor carries more of the item than before.
    ItemAcquired {
        actor: ActorId,
        definition: ItemDefinitionId,
    },
    QuestStarted(QuestId),
    QuestChanged(QuestId),
    VariableChanged(VariableId),
    DialogueCompleted(DialogueId),
}
impl WorldSignal {
    fn validate(&self, content: &GameContent) -> Result<()> {
        match self {
            Self::Entered { actor, area }
            | Self::Exited { actor, area }
            | Self::Arrived { actor, area } => {
                content.character(*actor)?;
                content.area(*area)?
            }
            Self::MoveFailed(actor) | Self::Died(actor) | Self::LeveledUp(actor) => {
                content.character(*actor)?;
            }
            Self::ItemAcquired { actor, definition } => {
                content.character(*actor)?;
                content.items.item(*definition)?;
            }
            Self::QuestStarted(id) | Self::QuestChanged(id) => {
                content.quest(*id)?;
            }
            Self::VariableChanged(id) => {
                content.variable(*id)?;
            }
            Self::DialogueCompleted(id) => {
                content.dialogue_contract(*id)?;
            }
        }
        Ok(())
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerRepeat {
    #[default]
    Once,
    Always,
    Cooldown {
        millis: u64,
    },
}
/// When any of `on` happens and the condition holds, the actions run as one unit: either
/// all of them take effect or none do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerDefinition {
    pub id: TriggerId,
    /// Who the condition and actions are about.
    pub player: ActorId,
    /// The other party, for rules that name `Speaker`.
    #[serde(default)]
    pub speaker: Option<ActorId>,
    pub on: BTreeSet<WorldSignal>,
    #[serde(default)]
    pub condition: Option<Condition>,
    pub actions: Vec<Action>,
    #[serde(default)]
    pub repeat: TriggerRepeat,
}
impl TriggerDefinition {
    pub fn speaker(&self) -> ActorId {
        self.speaker.unwrap_or(self.player)
    }
    pub fn validate(&self) -> Result<()> {
        require(
            self.speaker != Some(self.player),
            "trigger speaker must differ from its player",
        )?;
        require(!self.on.is_empty(), "a trigger listens for something")?;
        require(!self.actions.is_empty(), "a trigger does something")?;
        if let TriggerRepeat::Cooldown { millis } = self.repeat {
            require(
                millis > 0 && millis <= i64::MAX as u64,
                "invalid trigger cooldown",
            )?;
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectState {
    pub id: ObjectId,
    pub locked: bool,
    pub open: bool,
    pub destroyed: bool,
}
impl ObjectState {
    pub fn initial(d: &ObjectDefinition) -> Self {
        Self {
            id: d.id,
            locked: d.locked,
            open: false,
            destroyed: false,
        }
    }
}
/// The areas an actor was last reported inside. Kept so that loading a save does not
/// announce entering them again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationState {
    pub actor: ActorId,
    pub areas: BTreeSet<AreaId>,
}
/// A request for the engine to walk an actor into an area. It ends when the actor is
/// reported inside that area, when the engine gives up, or when its time runs out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Movement {
    pub actor: ActorId,
    pub to: AreaId,
    /// Tells this request apart from an earlier one for the same actor.
    pub request: u64,
    pub deadline: Option<GameTime>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerState {
    pub id: TriggerId,
    pub fired: u64,
    pub last_fired: Option<GameTime>,
}
impl TriggerState {
    pub fn initial(id: TriggerId) -> Self {
        Self {
            id,
            fired: 0,
            last_fired: None,
        }
    }
    pub fn eligible(&self, d: &TriggerDefinition, now: GameTime) -> bool {
        match d.repeat {
            TriggerRepeat::Once => self.fired == 0,
            TriggerRepeat::Always => true,
            TriggerRepeat::Cooldown { millis } => self
                .last_fired
                .is_none_or(|t| now.0.saturating_sub(t.0) >= millis),
        }
    }
}
/// Work a command gave rise to, carried out straight after it, each piece on its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pending {
    Signal(WorldSignal),
    /// A conversation asked for by an action. Starting it afterwards lets its graph be loaded.
    Start {
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    },
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldState {
    #[serde(with = "crate::keyed::list")]
    pub objects: BTreeMap<ObjectId, ObjectState>,
    #[serde(with = "crate::keyed::list")]
    pub locations: BTreeMap<ActorId, LocationState>,
    #[serde(with = "crate::keyed::list")]
    pub triggers: BTreeMap<TriggerId, TriggerState>,
    #[serde(with = "crate::keyed::list")]
    pub movements: BTreeMap<ActorId, Movement>,
    /// Oldest first.
    pub pending: VecDeque<Pending>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldCommand {
    /// The engine reports where an actor is and which areas contain it.
    Observe {
        actor: ActorId,
        position: Position,
        areas: BTreeSet<AreaId>,
    },
    /// The engine could not complete the movement it was asked for.
    MoveFailed {
        actor: ActorId,
        request: u64,
    },
    /// The engine records where actors stand, e.g. for a save. Which areas they are in is
    /// left alone: that changes only with `Observe`, so recording never sets off a trigger.
    Record {
        positions: BTreeMap<ActorId, Position>,
    },
    Open {
        object: ObjectId,
    },
    Close {
        object: ObjectId,
    },
    SetLocked {
        object: ObjectId,
        locked: bool,
    },
    Destroy {
        object: ObjectId,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldEvent {
    Entered {
        actor: ActorId,
        area: AreaId,
    },
    Exited {
        actor: ActorId,
        area: AreaId,
    },
    ObjectChanged(ObjectId),
    MoveRequested(Movement),
    Arrived {
        actor: ActorId,
        area: AreaId,
    },
    MoveFailed {
        actor: ActorId,
    },
    TriggerFired(TriggerId),
    /// The trigger's actions could not all be applied; none of them were.
    TriggerFailed {
        trigger: TriggerId,
        reason: String,
    },
    DialogueRefused {
        dialogue: DialogueId,
        reason: String,
    },
    /// Queued work could not be carried out and was dropped, so the queue moves on.
    WorkFailed {
        reason: String,
    },
}
impl GameContent {
    pub fn object(&self, id: ObjectId) -> Result<&ObjectDefinition> {
        self.game
            .world
            .objects
            .get(&id)
            .ok_or_else(|| Invalid("unknown object".into()).into())
    }
    /// Checks that the content declares the area.
    pub fn area(&self, id: AreaId) -> Result<()> {
        require(
            self.game.world.areas.contains(&id),
            &format!("unknown area {id}"),
        )
        .map_err(Into::into)
    }
    pub fn trigger(&self, id: TriggerId) -> Result<&TriggerDefinition> {
        self.game
            .world
            .triggers
            .get(&id)
            .ok_or_else(|| Invalid("unknown trigger".into()).into())
    }
    pub(crate) fn validate_world(&self) -> Result<()> {
        let w = &self.game.world;
        crate::content::filed(&w.objects, "objects")?;
        for d in w.objects.values() {
            d.name.validate()?;
        }
        crate::content::filed(&w.triggers, "triggers")?;
        for d in w.triggers.values() {
            crate::content::within(format!("trigger {}", d.id), || {
                d.validate()?;
                self.character(d.player)?;
                if let Some(speaker) = d.speaker {
                    self.character(speaker)?;
                }
                for signal in &d.on {
                    signal.validate(self)?;
                }
                if let Some(condition) = &d.condition {
                    self.validate_condition(condition)?;
                }
                for action in &d.actions {
                    self.validate_action(action, 0)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}
/// Which triggers listen for which signal, built once from the loaded definitions.
#[derive(Debug, Default)]
pub struct TriggerIndex {
    subscribers: BTreeMap<WorldSignal, BTreeSet<TriggerId>>,
    /// Actors whose comings and goings some trigger listens for.
    watched: BTreeSet<ActorId>,
}
impl TriggerIndex {
    pub fn build(content: &GameContent) -> Self {
        let mut index = Self::default();
        for d in content.game.world.triggers.values() {
            for signal in &d.on {
                if let WorldSignal::Entered { actor, .. } | WorldSignal::Exited { actor, .. } =
                    signal
                {
                    index.watched.insert(*actor);
                }
                index
                    .subscribers
                    .entry(signal.clone())
                    .or_default()
                    .insert(d.id);
            }
        }
        index
    }
    pub fn subscribed(&self, signal: &WorldSignal) -> bool {
        self.subscribers.contains_key(signal)
    }
    /// In identity order, so the same event always runs its triggers in the same order.
    pub fn subscribers(&self, signal: &WorldSignal) -> impl Iterator<Item = TriggerId> + '_ {
        self.subscribers.get(signal).into_iter().flatten().copied()
    }
    pub fn watches(&self, actor: ActorId) -> bool {
        self.watched.contains(&actor)
    }
}
impl WorldState {
    pub(crate) fn validate(&self, content: &GameContent, state: &SessionState) -> Result<()> {
        for work in &self.pending {
            match work {
                Pending::Signal(signal) => signal.validate(content)?,
                Pending::Start {
                    dialogue,
                    participant,
                    speaker,
                } => {
                    content.dialogue_contract(*dialogue)?;
                    state.actor(*participant)?;
                    state.actor(*speaker)?;
                }
            }
        }
        for o in self.objects.values() {
            state.check_object(content, o)?;
        }
        for l in self.locations.values() {
            state.check_location(content, l)?;
        }
        for t in self.triggers.values() {
            state.check_trigger(content, t)?;
        }
        for m in self.movements.values() {
            state.check_movement(content, m)?;
        }
        Ok(())
    }
}
