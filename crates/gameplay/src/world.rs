//! The logical world: objects and their locks, which areas actors are in, movement the
//! engine carries out, and triggers that react to what happens. Shapes and positions belong
//! to the engine; gameplay knows areas only by name and is told who is inside them.
use crate::Result;
use crate::actors::Position;
use crate::*;
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_AREA_OVERLAP: usize = 32;
pub const MAX_PENDING_EVENTS: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldDefinitions {
    pub objects: Vec<ObjectDefinition>,
    /// Areas this content refers to. The world supplies their shapes under the same names.
    pub areas: BTreeSet<AreaId>,
    pub triggers: Vec<TriggerDefinition>,
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
            Self::Entered { area, .. } | Self::Exited { area, .. } | Self::Arrived { area, .. } => {
                content.area(*area)?
            }
            Self::MoveFailed(_) | Self::Died(_) | Self::LeveledUp(_) => {}
            Self::ItemAcquired { definition, .. } => {
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
        require(
            !self.on.is_empty() && self.on.len() <= 32,
            "trigger listens for 1..32 events",
        )?;
        require(
            !self.actions.is_empty() && self.actions.len() <= 64,
            "trigger has 1..64 actions",
        )?;
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
/// Work accepted by a command and carried out by a later `ProcessNext`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pending {
    Signal(WorldSignal),
    /// A conversation asked for by an action. Starting it later lets its graph be loaded.
    Start {
        dialogue: DialogueId,
        participant: ActorId,
        speaker: ActorId,
    },
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldState {
    #[serde(with = "crate::state::keyed")]
    pub objects: BTreeMap<ObjectId, ObjectState>,
    #[serde(with = "crate::state::keyed")]
    pub locations: BTreeMap<ActorId, LocationState>,
    #[serde(with = "crate::state::keyed")]
    pub triggers: BTreeMap<TriggerId, TriggerState>,
    #[serde(with = "crate::state::keyed")]
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
    /// Carries out the oldest pending work, or a movement whose time ran out. Call again
    /// while work remains.
    ProcessNext,
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
}
impl GameContent {
    pub fn object(&self, id: ObjectId) -> Result<&ObjectDefinition> {
        crate::content::find(&self.game.world.objects, id, |v| v.id)
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
        crate::content::find(&self.game.world.triggers, id, |v| v.id)
            .ok_or_else(|| Invalid("unknown trigger".into()).into())
    }
    pub(crate) fn validate_world(&self) -> Result<()> {
        let w = &self.game.world;
        require(
            w.objects.len() <= 10000 && w.areas.len() <= 10000 && w.triggers.len() <= 10000,
            "world definition budget exceeded",
        )?;
        crate::content::ordered(&w.objects, |v| v.id, "objects")?;
        for d in &w.objects {
            d.name.validate()?;
        }
        crate::content::ordered(&w.triggers, |v| v.id, "triggers")?;
        for d in &w.triggers {
            d.validate()?;
            for signal in &d.on {
                signal.validate(self)?;
            }
            if let Some(condition) = &d.condition {
                self.validate_condition(condition)?;
            }
            let mut budget = 1024;
            for action in &d.actions {
                self.validate_action(action, 0, &mut budget)?;
            }
        }
        Ok(())
    }
}
/// Which triggers listen for which signal, built once from the loaded definitions.
#[derive(Debug, Default)]
pub struct TriggerIndex(BTreeMap<WorldSignal, BTreeSet<TriggerId>>);
impl TriggerIndex {
    pub fn build(content: &GameContent) -> Self {
        let mut index = BTreeMap::<_, BTreeSet<_>>::new();
        for d in &content.game.world.triggers {
            for signal in &d.on {
                index.entry(signal.clone()).or_default().insert(d.id);
            }
        }
        Self(index)
    }
    pub fn subscribed(&self, signal: &WorldSignal) -> bool {
        self.0.contains_key(signal)
    }
    /// In identity order, so the same event always runs its triggers in the same order.
    pub fn subscribers(&self, signal: &WorldSignal) -> impl Iterator<Item = TriggerId> + '_ {
        self.0.get(signal).into_iter().flatten().copied()
    }
}
impl WorldState {
    pub(crate) fn validate(&self, content: &GameContent, state: &SessionState) -> Result<()> {
        require(
            self.pending.len() <= MAX_PENDING_EVENTS,
            "world state budget exceeded",
        )?;
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
