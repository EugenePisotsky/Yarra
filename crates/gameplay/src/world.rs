//! Durable logical world contracts. Engine adapters own navigation and physical realization.
use crate::Result;
use crate::actors::Position;
use crate::*;
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_AREA_OVERLAP: usize = 32;
pub const MAX_PENDING_EVENTS: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldDefinitions {
    pub objects: Vec<ObjectDefinition>,
    pub areas: Vec<AreaDefinition>,
    pub triggers: Vec<TriggerDefinition>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectKind {
    Door,
    Container { inventory: InventoryId },
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AreaDefinition {
    pub id: AreaId,
    pub min: Position,
    pub max: Position,
}
impl AreaDefinition {
    pub fn validate(&self) -> Result<()> {
        require(
            (0..3).all(|i| self.min.millimetres[i] < self.max.millimetres[i]),
            "empty area bounds",
        )?;
        Ok(())
    }
    /// Half-open bounds give adjacent areas an unambiguous shared boundary.
    pub fn contains(&self, p: &Position) -> bool {
        (0..3).all(|i| {
            self.min.millimetres[i] <= p.millimetres[i]
                && p.millimetres[i] < self.max.millimetres[i]
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WorldSignal {
    ItemAcquired {
        actor: ActorId,
        definition: ItemDefinitionId,
    },
    QuestStarted(QuestId),
    Quest(QuestId),
    Actor(ActorId),
    Fact(Key),
    History(dialogue::HistoryKey),
    Claim(dialogue::ClaimKey),
    Relationship(actors::RelationshipKey),
    Entered {
        actor: ActorId,
        area: AreaId,
    },
    Exited {
        actor: ActorId,
        area: AreaId,
    },
}
impl WorldSignal {
    fn validate(&self, content: &GameContent) -> Result<()> {
        match self {
            Self::ItemAcquired { definition, .. } => {
                content.items.item(*definition)?;
            }
            Self::QuestStarted(id) | Self::Quest(id) => {
                content.quest(*id)?;
            }
            Self::Fact(key) => require(content.game.facts.contains(key), "unknown trigger fact")?,
            Self::Entered { area, .. } | Self::Exited { area, .. } => {
                content.area(*area)?;
            }
            Self::History(key) => {
                key.scope.validate()?;
                require(
                    content
                        .dialogue_contract(key.dialogue)?
                        .history_scope
                        .accepts(key.scope),
                    "trigger history scope mismatch",
                )?;
            }
            Self::Claim(key) => {
                key.scope.validate()?;
                require(
                    content.claim(key.claim)?.scope.accepts(key.scope),
                    "trigger claim scope mismatch",
                )?;
            }
            Self::Relationship(key) => actors::Relationship::neutral(*key).validate()?,
            Self::Actor(_) => {}
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerActivation {
    /// Reconcile current state whenever any condition dependency changes.
    Maintained,
    /// Edge semantics: only these observations can initiate a sequence.
    Events(BTreeSet<WorldSignal>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerRepeat {
    OnceSucceeded,
    Always,
    Cooldown { millis: u64 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SequenceStep {
    Apply(Vec<Action>),
    Move {
        actor: Participant,
        destination: Position,
        timeout_ms: u64,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerDefinition {
    pub id: TriggerId,
    /// Placed actor bindings. Spawned/dynamic instances can get a separate binding layer later.
    pub participant: ActorId,
    pub speaker: ActorId,
    pub activation: TriggerActivation,
    pub condition: Condition,
    pub repeat: TriggerRepeat,
    pub steps: Vec<SequenceStep>,
}
impl TriggerDefinition {
    pub fn validate(&self) -> Result<()> {
        require(
            self.participant != self.speaker,
            "trigger requires distinct actors",
        )?;
        require(
            !self.steps.is_empty() && self.steps.len() <= 32,
            "sequence requires 1..32 steps",
        )?;
        self.condition.visit(&mut |_| Ok(()))?;
        if let TriggerActivation::Events(v) = &self.activation {
            require(
                !v.is_empty() && v.len() <= 32,
                "event trigger requires 1..32 subscriptions",
            )?;
        }
        if let TriggerRepeat::Cooldown { millis } = self.repeat {
            require(
                millis > 0 && millis <= i64::MAX as u64,
                "invalid trigger cooldown",
            )?;
        }
        let mut count = 0;
        for step in &self.steps {
            match step {
                SequenceStep::Move { timeout_ms, .. } => require(
                    *timeout_ms > 0 && *timeout_ms <= 86_400_000,
                    "movement timeout must be 1ms..1 day",
                )?,
                SequenceStep::Apply(actions) => {
                    require(!actions.is_empty(), "empty sequence step")?;
                    for action in actions {
                        action.visit(&mut |_| {
                            count += 1;
                            require(count <= 1024, "sequence action budget exceeded")?;
                            Ok(())
                        })?;
                    }
                }
            }
        }
        Ok(())
    }
    pub fn content_dependencies(&self, request: &mut ContentRequest) -> Result<()> {
        self.condition.content_dependencies(request)?;
        if let TriggerActivation::Events(signals) = &self.activation {
            for signal in signals {
                match signal {
                    WorldSignal::Entered { area, .. } | WorldSignal::Exited { area, .. } => {
                        request.areas.insert(*area);
                    }
                    WorldSignal::ItemAcquired { definition, .. } => {
                        request.items.insert(*definition);
                    }
                    WorldSignal::QuestStarted(id) | WorldSignal::Quest(id) => {
                        request.quests.insert(*id);
                    }
                    WorldSignal::Fact(id) => {
                        request.facts.insert(id.clone());
                    }
                    WorldSignal::History(key) => {
                        request.dialogue_contracts.insert(key.dialogue);
                    }
                    WorldSignal::Claim(key) => {
                        request.claims.insert(key.claim);
                    }
                    _ => {}
                }
            }
        }
        for step in &self.steps {
            if let SequenceStep::Apply(actions) = step {
                for action in actions {
                    action.content_dependencies(request)?;
                }
            }
        }
        Ok(())
    }
    pub fn subscriptions(&self, content: &GameContent) -> Result<BTreeSet<WorldSignal>> {
        if let TriggerActivation::Events(v) = &self.activation {
            return Ok(v.clone());
        }
        let mut out = BTreeSet::new();
        self.condition
            .visit_resolved(&|id| content.predicate(id), &mut |c| {
                match c {
                    Condition::InsideArea { area } => {
                        out.insert(WorldSignal::Entered {
                            actor: self.participant,
                            area: *area,
                        });
                        out.insert(WorldSignal::Exited {
                            actor: self.participant,
                            area: *area,
                        });
                    }
                    Condition::HasItem { .. } | Condition::SkillExperience { .. } => {
                        out.insert(WorldSignal::Actor(self.participant));
                    }
                    Condition::QuestStatus { quest, .. }
                    | Condition::ObjectiveCompleted { quest, .. } => {
                        out.insert(WorldSignal::Quest(*quest));
                    }
                    Condition::Fact { key, .. } => {
                        out.insert(WorldSignal::Fact(key.clone()));
                    }
                    Condition::History { dialogue, .. } => {
                        out.insert(WorldSignal::History(
                            content
                                .dialogue_contract(*dialogue)?
                                .history_key(self.participant, self.speaker),
                        ));
                    }
                    Condition::Claimed { claim, .. } => {
                        out.insert(WorldSignal::Claim(
                            content.claim(*claim)?.key(self.participant, self.speaker),
                        ));
                    }
                    Condition::Relationship { from, to, .. } => {
                        out.insert(WorldSignal::Relationship(actors::RelationshipKey {
                            from: from.resolve(self.participant, self.speaker),
                            to: to.resolve(self.participant, self.speaker),
                        }));
                    }
                    _ => {}
                }
                Ok(())
            })?;
        require(
            !out.is_empty() && out.len() <= 64,
            "maintained trigger needs 1..64 dependencies",
        )?;
        Ok(out)
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationState {
    pub actor: ActorId,
    /// Adapter observations are ordered per actor and retried with the same sequence/position.
    pub observation: u64,
    pub last_observed: Option<Position>,
    pub areas: BTreeSet<AreaId>,
}
impl LocationState {
    pub fn initial(actor: ActorId) -> Self {
        Self {
            actor,
            observation: 0,
            last_observed: None,
            areas: BTreeSet::new(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ActionId {
    pub trigger: TriggerId,
    pub run: u64,
    pub step: u16,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MovementPhase {
    Accepted,
    Running,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveRequest {
    pub id: ActionId,
    pub actor: ActorId,
    pub destination: Position,
    pub deadline: GameTime,
    pub phase: MovementPhase,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveResult {
    Arrived { position: Position },
    Failed { reason: Key },
    Cancelled,
    TimedOut,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SequenceStatus {
    Idle,
    Running,
    Succeeded,
    Failed { reason: Key },
    Cancelled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerState {
    pub id: TriggerId,
    pub run: u64,
    pub step: u16,
    pub status: SequenceStatus,
    pub last_started: Option<GameTime>,
    pub successes: u64,
    pub consecutive_failures: u8,
    pub diagnostic: Option<String>,
    pub movement: Option<MoveRequest>,
    /// Only the current run's bounded receipts are retained; older run IDs are rejected.
    pub receipts: Vec<(ActionId, MoveResult)>,
}
impl TriggerState {
    pub fn initial(id: TriggerId) -> Self {
        Self {
            id,
            run: 0,
            step: 0,
            status: SequenceStatus::Idle,
            last_started: None,
            successes: 0,
            consecutive_failures: 0,
            diagnostic: None,
            movement: None,
            receipts: vec![],
        }
    }
    pub fn eligible(&self, d: &TriggerDefinition, now: GameTime) -> bool {
        if self.status == SequenceStatus::Running || self.consecutive_failures >= 3 {
            return false;
        }
        match d.repeat {
            TriggerRepeat::OnceSucceeded => self.successes == 0,
            TriggerRepeat::Always => true,
            TriggerRepeat::Cooldown { millis } => self
                .last_started
                .is_none_or(|t| now.0.saturating_sub(t.0) >= millis),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId {
    pub generation: u64,
    pub ordinal: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingEvent {
    pub id: EventId,
    pub signal: WorldSignal,
    pub after: Option<TriggerId>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorldState {
    pub objects: Vec<ObjectState>,
    pub locations: Vec<LocationState>,
    pub triggers: Vec<TriggerState>,
    /// Tool import/export only. Runtime commands use the transaction queue port.
    pub pending: Vec<PendingEvent>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldCommand {
    ObservePosition {
        actor: ActorId,
        observation: u64,
        position: Position,
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
    /// One trigger delivery (or timeout) per commit. Call again while work remains.
    ProcessNext,
    RetryTrigger {
        trigger: TriggerId,
    },
    StartMove {
        id: ActionId,
    },
    FinishMove {
        id: ActionId,
        result: MoveResult,
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
    MoveRequested(MoveRequest),
    MoveFinished {
        id: ActionId,
        result: MoveResult,
    },
    SequenceChanged {
        trigger: TriggerId,
        status: SequenceStatus,
    },
    DeliveryProcessed(EventId),
}
impl GameContent {
    pub fn object(&self, id: ObjectId) -> Result<&ObjectDefinition> {
        self.game
            .world
            .objects
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| Invalid("unknown object".into()).into())
    }
    pub fn area(&self, id: AreaId) -> Result<&AreaDefinition> {
        self.game
            .world
            .areas
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| Invalid("unknown area".into()).into())
    }
    pub fn trigger(&self, id: TriggerId) -> Result<&TriggerDefinition> {
        self.game
            .world
            .triggers
            .iter()
            .find(|d| d.id == id)
            .ok_or_else(|| Invalid("unknown trigger".into()).into())
    }
    pub(crate) fn validate_world(&self) -> Result<()> {
        let w = &self.game.world;
        require(
            w.objects.len() <= 10000 && w.areas.len() <= 10000 && w.triggers.len() <= 10000,
            "world definition budget exceeded",
        )?;
        let mut ids = BTreeSet::new();
        for d in &w.objects {
            require(ids.insert(d.id), "duplicate object")?;
            d.name.validate()?;
        }
        let mut ids = BTreeSet::new();
        for d in &w.areas {
            require(ids.insert(d.id), "duplicate area")?;
            d.validate()?;
        }
        let mut ids = BTreeSet::new();
        for d in &w.triggers {
            require(ids.insert(d.id), "duplicate trigger")?;
            d.validate()?;
            self.validate_condition(&d.condition)?;
            for signal in d.subscriptions(self)? {
                signal.validate(self)?;
            }
            for step in &d.steps {
                if let SequenceStep::Apply(actions) = step {
                    for action in actions {
                        self.validate_action(action, 0, &mut 1024)?;
                    }
                }
            }
        }
        Ok(())
    }
}
impl WorldState {
    pub fn object(&self, id: ObjectId) -> Result<&ObjectState> {
        self.objects
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Invalid("object state not resolved".into()).into())
    }
    pub fn location(&self, actor: ActorId) -> Result<&LocationState> {
        self.locations
            .iter()
            .find(|v| v.actor == actor)
            .ok_or_else(|| Invalid("location not resolved".into()).into())
    }
    pub fn trigger(&self, id: TriggerId) -> Result<&TriggerState> {
        self.triggers
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| Invalid("trigger state not resolved".into()).into())
    }
    pub(crate) fn validate(&self, content: &GameContent, state: &SessionState) -> Result<()> {
        require(
            self.objects.len() <= 10000
                && self.locations.len() <= 10000
                && self.triggers.len() <= 10000
                && self.pending.len() <= MAX_PENDING_EVENTS,
            "world state budget exceeded",
        )?;
        let mut events = BTreeSet::new();
        for event in &self.pending {
            require(
                event.id.generation > 0
                    && event.id.generation < state.generation
                    && event.id.ordinal < 256
                    && events.insert(event.id),
                "invalid pending event identity",
            )?;
            if let Some(id) = event.after {
                content.trigger(id)?;
            }
        }
        let mut ids = BTreeSet::new();
        for o in &self.objects {
            let definition = content.object(o.id)?;
            if let ObjectKind::Container { inventory } = definition.kind
                && let Some(bag) = state.inventories.iter().find(|i| i.id == inventory)
            {
                require(
                    bag.owner == OwnerRef::new("object", OwnerId(o.id.0))?
                        && bag.role == "contents",
                    "container inventory ownership mismatch",
                )?;
            }
            require(
                ids.insert(o.id) && !(o.open && (o.locked || o.destroyed)),
                "invalid object state",
            )?;
        }
        let mut ids = BTreeSet::new();
        for l in &self.locations {
            state.actor(l.actor)?;
            require(
                ids.insert(l.actor)
                    && l.areas.len() <= MAX_AREA_OVERLAP
                    && l.observation <= i64::MAX as u64
                    && (l.observation == 0) == l.last_observed.is_none(),
                "invalid location state",
            )?;
            for id in &l.areas {
                content.area(*id)?;
            }
        }
        let mut ids = BTreeSet::new();
        for t in &self.triggers {
            let d = content.trigger(t.id)?;
            state.actor(d.participant)?;
            state.actor(d.speaker)?;
            require(
                ids.insert(t.id)
                    && t.run <= i64::MAX as u64
                    && t.successes <= t.run
                    && usize::from(t.step) <= d.steps.len()
                    && t.receipts.len() <= 32
                    && t.consecutive_failures <= 3
                    && t.diagnostic.as_ref().is_none_or(|d| d.len() <= 2048)
                    && t.last_started.is_none_or(|time| time <= state.time),
                "invalid sequence progress",
            )?;
            require(
                (t.run == 0) == (t.status == SequenceStatus::Idle)
                    && (t.run == 0) == t.last_started.is_none(),
                "invalid sequence run/status",
            )?;
            if t.status == SequenceStatus::Idle {
                require(
                    t.step == 0 && t.receipts.is_empty() && t.successes == 0,
                    "invalid idle sequence",
                )?;
            }
            if t.status == SequenceStatus::Succeeded {
                require(
                    usize::from(t.step) == d.steps.len() && t.successes > 0,
                    "invalid successful sequence",
                )?;
            }
            let mut receipts = BTreeSet::new();
            for (id, result) in &t.receipts {
                require(
                    id.trigger == t.id
                        && id.run == t.run
                        && id.step <= t.step
                        && receipts.insert(*id),
                    "invalid movement receipt",
                )?;
                let Some(SequenceStep::Move { destination, .. }) =
                    d.steps.get(usize::from(id.step))
                else {
                    return Err(Invalid("receipt is not a movement step".into()).into());
                };
                if let MoveResult::Arrived { position } = result {
                    require(position == destination, "invalid arrival receipt")?;
                }
            }
            require(
                (t.status == SequenceStatus::Running) == t.movement.is_some(),
                "running sequence must await movement",
            )?;
            if let Some(m) = &t.movement {
                require(
                    !receipts.contains(&m.id)
                        && m.id
                            == ActionId {
                                trigger: t.id,
                                run: t.run,
                                step: t.step,
                            }
                        && m.deadline.0 <= i64::MAX as u64,
                    "invalid pending movement identity",
                )?;
                match d.steps.get(usize::from(t.step)) {
                    Some(SequenceStep::Move {
                        actor, destination, ..
                    }) => require(
                        m.actor == actor.resolve(d.participant, d.speaker)
                            && &m.destination == destination,
                        "movement plan mismatch",
                    )?,
                    _ => return Err(Invalid("movement step missing".into()).into()),
                }
            }
        }
        Ok(())
    }
}
