//! First playable gameplay slice, opt-in with `--story DIR`: a guard and a gate near the
//! start, a conversation, triggers that react to where the party walks, and quick save/load.
//! The rules run in `gameplay`; this module only publishes the authored project, forwards
//! input and what the engine observes as commands, and shows read models.
use game_content::{ContentRepository, LoadedProject};
use game_types::{ActorId, AreaId, BoundText, ItemDefinitionId, Key, ObjectId, QuestId, TextRef};
use gameplay::actors::Position;
use gameplay::dialogue::{Mode, Token};
use gameplay::{
    Command, ContentSource, ConversationKey, ConversationView, Driver, GameEvent, GameSession,
    Movement, WorldCommand, WorldEvent, quests,
};
use localization::Arguments;
use save::{SaveDirectory, SaveSlot};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

mod scene;
pub(crate) use scene::install;

/// Starting conditions for the slice, relative to the project directory.
const SCENARIO: &str = "scenarios/island.ron";
const HERO: ActorId = ActorId::named("hero");
const GUARD: ActorId = ActorId::named("guard");
const MIRA: ActorId = ActorId::named("mira");
const DUMMY: ActorId = ActorId::named("dummy");
const POTION: ItemDefinitionId = ItemDefinitionId::named("healing_potion");
const GATE: ObjectId = ObjectId::named("guard/old_gate");
const GATE_QUEST: QuestId = QuestId::named("guard/gate");
const GATE_KEY: ItemDefinitionId = ItemDefinitionId::named("old_gate_key");
/// Game time moves in steps of this many milliseconds.
const TIME_STEP_MS: u64 = 100;

fn millimetres(metres: [f64; 3]) -> Position {
    Position {
        millimetres: metres.map(|metres| (metres * 1000.).round() as i64),
    }
}

/// Fluent wraps inserted values in directional isolation marks; the UI font has no glyphs
/// for them and the slice shows left-to-right text only.
fn plain(text: String) -> String {
    text.replace(['\u{2068}', '\u{2069}'], "")
}

/// What the dialogue panel shows. With no choices, the line waits to be acknowledged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Panel {
    pub speaker: String,
    pub text: String,
    pub choices: Vec<String>,
}

/// A line spoken while play goes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bark {
    pub speaker: String,
    pub text: String,
}

pub(crate) struct Story {
    project: LoadedProject,
    driver: Driver<ContentRepository>,
    bundle: PathBuf,
    saves: SaveDirectory,
    locale: String,
    /// The ambient line being spoken, how many seconds it has been up, and how long it stays
    /// once that is worked out.
    ambient_line: Option<(ConversationKey, Token)>,
    ambient_shown: f32,
    ambient_duration: Option<f32>,
    /// Counts loads, so the scene moves actors to where the save left them.
    loads: u64,
    guard_name: String,
    /// Changes whenever anything a read model shows may have changed.
    revision: u64,
    /// Outcome of the last save/load or rejected input, shown until replaced.
    pub notice: String,
}
impl Story {
    /// Publishes the authored project into `work` and starts its scenario against the bundle.
    pub fn open(source: &Path, work: &Path) -> Result<Self, String> {
        let text = |e: &dyn std::fmt::Display| e.to_string();
        let project =
            LoadedProject::load_directory_with_scenario(source, SCENARIO).map_err(|e| text(&e))?;
        std::fs::create_dir_all(work).map_err(|e| text(&e))?;
        let bundle = work.join("content.sqlite");
        // Publication never replaces a bundle; this one is a private copy for the session.
        match std::fs::remove_file(&bundle) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(text(&e)),
            _ => {}
        }
        project.build(&bundle).map_err(|e| text(&e))?;
        let mut session = GameSession::new(
            ContentRepository::open(&bundle).map_err(|e| text(&e))?,
            project.start().map_err(|e| text(&e))?.into_state(),
        )
        .map_err(|e| text(&e))?;
        for step in &project.scenario().steps {
            step.apply(&mut session).map_err(|e| text(&e))?;
        }
        let saves = SaveDirectory::new(work.join("saves"), 3).map_err(|e| text(&e))?;
        let locale = project.source_locale().to_owned();
        let driver = Driver::new(session, TIME_STEP_MS, 0).map_err(|e| text(&e))?;
        let mut story = Self {
            project,
            driver,
            bundle,
            saves,
            locale,
            ambient_line: None,
            ambient_shown: 0.,
            ambient_duration: None,
            loads: 0,
            guard_name: String::new(),
            revision: 0,
            notice: String::new(),
        };
        story.guard_name = story.actor_name(GUARD);
        Ok(story)
    }
    fn format(&self, text: &TextRef) -> String {
        match self
            .project
            .localization()
            .format(&self.locale, text, &Arguments::new())
        {
            Ok(text) => plain(text.value),
            Err(error) => format!("<{error}>"),
        }
    }
    fn bound(&self, text: &BoundText) -> String {
        match self.project.localization().format_bound(&self.locale, text) {
            Ok(text) => plain(text.value),
            Err(error) => format!("<{error}>"),
        }
    }
    fn actor_name(&self, id: ActorId) -> String {
        let state = self.driver.state();
        let Ok(actor) = state.actor(id) else {
            return String::new();
        };
        match (
            &actor.name_override,
            self.project.content().template(actor.template),
        ) {
            (Some(name), _) => self.format(name),
            (None, Ok(template)) => self.format(&template.name),
            _ => String::new(),
        }
    }
    pub fn guard_name(&self) -> &str {
        &self.guard_name
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Whether a conversation has the player's attention. The world waits while one does.
    pub fn in_conversation(&self) -> bool {
        self.driver.state().floor.current(Mode::Blocking).is_some()
    }
    pub fn gate_locked(&self) -> bool {
        let session = self.driver.session();
        session
            .state()
            .object(session.content(), GATE)
            .is_ok_and(|gate| gate.locked)
    }
    /// One line for the corner of the screen: the quest, what the player carries for it and
    /// the named areas the player stands in.
    pub fn summary(&self) -> String {
        let state = self.driver.state();
        let title = self
            .project
            .content()
            .quest(GATE_QUEST)
            .map(|q| self.format(&q.title))
            .unwrap_or_default();
        let status = match state.quest(GATE_QUEST).status {
            quests::Status::NotStarted => "not started",
            quests::Status::Active => "active",
            quests::Status::Completed => "completed",
            quests::Status::Failed => "failed",
        };
        let keys: u32 = state.carried(HERO).map_or(0, |bag| {
            bag.entries
                .iter()
                .filter(|e| e.definition == GATE_KEY)
                .map(|e| e.quantity)
                .sum()
        });
        let gate = if self.gate_locked() {
            "locked"
        } else {
            "unlocked"
        };
        let mut summary = format!("{title}: {status}  |  Gate key: {keys}  |  Gate: {gate}");
        let areas: Vec<String> = state.areas(HERO).iter().map(AreaId::to_string).collect();
        if !areas.is_empty() {
            summary.push_str(&format!("  |  In: {}", areas.join(", ")));
        }
        summary
    }

    fn run(&mut self, command: Command) -> bool {
        self.revision += 1;
        match self.driver.submit(command) {
            Ok(outcome) => {
                for event in outcome.events {
                    self.note(event);
                }
                true
            }
            Err(error) => {
                self.notice = error.to_string();
                false
            }
        }
    }
    fn note(&mut self, event: GameEvent) {
        match event {
            GameEvent::World(WorldEvent::TriggerFailed { trigger, reason }) => {
                self.notice = format!("Trigger {trigger} failed: {reason}");
            }
            GameEvent::World(WorldEvent::DialogueRefused { dialogue, reason }) => {
                self.notice = format!("Conversation {dialogue} did not start: {reason}");
            }
            GameEvent::World(WorldEvent::WorkFailed { reason }) => {
                self.notice = format!("Queued work failed: {reason}");
            }
            _ => {}
        }
    }
    /// The conversation of one kind that has the floor, as it stands now.
    fn current(&mut self, mode: Mode) -> Option<ConversationView> {
        let key = self.driver.state().floor.current(mode)?;
        match self.driver.session_mut().conversation_view(key) {
            Ok(view) => Some(view),
            Err(error) => {
                self.notice = error.to_string();
                None
            }
        }
    }

    /// Something the guard can be asked about now, with its label.
    pub fn topic(&self) -> Option<(Key, String)> {
        let preview = self
            .driver
            .session()
            .preview_interaction(HERO, GUARD)
            .ok()?;
        let entry = preview.topics().next()?;
        Some((entry.rule.clone(), self.format(entry.topic.as_ref()?)))
    }
    /// Starts or resumes the conversation the guard's entry rules select, or the one
    /// behind a topic.
    pub fn talk(&mut self, topic: Option<Key>) {
        if self.in_conversation() {
            return;
        }
        let accepted = self.run(Command::Talk {
            participant: HERO,
            speaker: GUARD,
            topic,
            bindings: Default::default(),
        });
        if accepted {
            self.notice.clear();
        }
    }
    /// The current line or choices, or `None` once the conversation has ended.
    pub fn panel(&mut self) -> Option<Panel> {
        let view = self.current(Mode::Blocking)?;
        Some(match &view.line {
            Some(line) => Panel {
                speaker: self.actor_name(line.speaker),
                text: self.bound(&line.text),
                choices: vec![],
            },
            None => Panel {
                speaker: self.actor_name(HERO),
                text: String::new(),
                choices: view.choices.iter().map(|c| self.bound(&c.text)).collect(),
            },
        })
    }
    /// The line of the ambient conversation being spoken, if there is one.
    pub fn bark(&mut self) -> Option<Bark> {
        let view = self.current(Mode::Ambient)?;
        let line = view.line.as_ref()?;
        Some(Bark {
            speaker: self.actor_name(line.speaker),
            text: self.bound(&line.text),
        })
    }
    /// Acknowledge the line being shown.
    pub fn advance(&mut self) {
        let Some(view) = self.current(Mode::Blocking) else {
            return;
        };
        if view.line.is_some() {
            self.run(Command::AdvanceLine {
                key: view.key,
                expected: view.token,
            });
        }
    }
    /// Pick the choice at this position in the list the panel showed.
    pub fn choose(&mut self, index: usize) {
        let Some(view) = self.current(Mode::Blocking) else {
            return;
        };
        if view.line.is_some() {
            return;
        }
        if let Some(choice) = view.choices.get(index) {
            let (key, choice) = (view.key, choice.id.clone());
            self.run(Command::Choose {
                expected: view.token,
                dialogue: key.dialogue,
                participant: key.participant,
                speaker: key.speaker,
                choice,
            });
        }
    }
    /// Walk away mid-conversation. The guard remembers that it was interrupted.
    pub fn leave(&mut self) {
        if let Some(view) = self.current(Mode::Blocking) {
            self.run(Command::InterruptDialogue {
                key: view.key,
                expected: view.token,
            });
        }
    }

    /// Lets `seconds` of play pass: game time moves on, a walk can run out of time and an
    /// ambient line gives way to the next. The rules hold the world still while a blocking
    /// conversation is on screen; the ambient line waits with it.
    pub fn tick(&mut self, seconds: f32) {
        let seconds = seconds.clamp(0., 1.);
        match self.driver.elapse(Duration::from_secs_f32(seconds)) {
            Ok(Some(outcome)) => {
                // The clock alone changes nothing on screen, unless the hero is busy with
                // something whose progress is shown.
                let quiet = outcome.events.iter().all(|e| *e == GameEvent::TimeAdvanced);
                if !quiet || self.busy() {
                    self.revision += 1;
                }
                for event in outcome.events {
                    self.note(event);
                }
            }
            Ok(None) => {}
            Err(error) => self.notice = error.to_string(),
        }
        if self.in_conversation() {
            return;
        }
        let state = self.driver.state();
        let line = state
            .floor
            .current(Mode::Ambient)
            .and_then(|key| Some((key, state.conversation(key).ok()?.token)));
        if self.ambient_line != line {
            self.ambient_line = line;
            self.ambient_shown = 0.;
            self.ambient_duration = None;
        }
        let Some((key, token)) = line else {
            return;
        };
        self.ambient_shown += seconds;
        let duration = match self.ambient_duration {
            Some(duration) => duration,
            None => {
                // Long enough to read: a moment, plus time for each character.
                let length = self
                    .current(Mode::Ambient)
                    .and_then(|view| view.line)
                    .map_or(0, |line| self.bound(&line.text).chars().count());
                *self
                    .ambient_duration
                    .insert((1.5 + 0.06 * length as f32).min(8.))
            }
        };
        if self.ambient_shown >= duration {
            self.run(Command::AdvanceLine {
                key,
                expected: token,
            });
        }
    }

    /// The hero's sheet in one line: level, experience, resources, attack and the purse.
    pub fn character(&self) -> String {
        let state = self.driver.state();
        let Ok(hero) = state.actor(HERO) else {
            return String::new();
        };
        let rules = &self.driver.session().content().game.rules;
        let stat = |name: &str| Key::new(name).ok().and_then(|key| hero.stats.get(&key));
        let left = |name: &str| Key::new(name).ok().and_then(|key| hero.resources.get(&key));
        let mut line = format!(
            "{}: level {}  |  {} XP",
            self.actor_name(HERO),
            hero.level,
            state.party.experience
        );
        for (resource, maximum, label) in [
            ("health", "max-health", "Health"),
            ("stamina", "max-stamina", "Stamina"),
        ] {
            if let (Some(now), Some(cap)) = (left(resource), stat(maximum)) {
                line.push_str(&format!("  |  {label} {now}/{cap}"));
            }
        }
        if let Some(attack) = stat("attack") {
            line.push_str(&format!("  |  Attack {attack}"));
        }
        for (skill, rank) in &hero.skills {
            if let Ok(definition) = rules.skill(skill) {
                line.push_str(&format!("  |  {} {rank}", self.format(&definition.name)));
            }
        }
        line.push_str(&format!("  |  Gold {}", state.gold().unwrap_or(0)));
        line
    }
    /// Attribute and learning points the hero has not spent.
    pub fn points(&self) -> (u32, u32) {
        let hero = self.driver.state().actor(HERO);
        hero.map_or((0, 0), |h| (h.attribute_points, h.learning_points))
    }
    /// Puts one attribute point into a primary stat.
    pub fn spend(&mut self, stat: &str) {
        if let Ok(stat) = Key::new(stat) {
            self.run(Command::SpendAttributePoint { actor: HERO, stat });
        }
    }
    /// Drinks a healing potion, if the hero carries one.
    pub fn drink(&mut self) {
        let carried = self.driver.state().carried(HERO);
        let potion = carried
            .ok()
            .and_then(|bag| bag.entries.iter().find(|e| e.definition == POTION))
            .map(|e| e.id);
        match potion {
            Some(item) => {
                self.run(Command::UseItem { actor: HERO, item });
            }
            None => self.notice = "No potions left.".into(),
        }
    }
    fn intend(&mut self, ability: &str, repeat: bool) {
        let Ok(ability) = Key::new(ability) else {
            return;
        };
        self.run(Command::Intend {
            actor: HERO,
            intent: gameplay::actors::Intent {
                ability,
                target: Some(DUMMY),
                repeat,
            },
            clear: false,
        });
    }
    /// Strikes the training dummy, again and again until told to stop.
    pub fn strike(&mut self) {
        if !self.busy() {
            self.intend("strike", true);
        }
    }
    /// Lines up one power strike after what the hero is doing.
    pub fn power_strike(&mut self) {
        self.intend("power-strike", false);
    }
    pub fn stop(&mut self) {
        if self.busy() {
            self.run(Command::Interrupt { actor: HERO });
        }
    }
    /// Whether the hero is doing something or has something lined up.
    pub fn busy(&self) -> bool {
        let hero = self.driver.state().actor(HERO);
        hero.is_ok_and(|h| h.acting.is_some() || !h.intents.is_empty())
    }
    /// The dummy's health, what the hero is in the middle of, and when the power strike is
    /// ready again.
    pub fn fight(&self) -> String {
        let state = self.driver.state();
        let (Ok(hero), Ok(dummy)) = (state.actor(HERO), state.actor(DUMMY)) else {
            return String::new();
        };
        let rules = &self.driver.session().content().game.rules;
        let seconds = |until: game_types::GameTime| {
            format!(
                "{:.1} s",
                until.0.saturating_sub(state.time.0) as f32 / 1000.
            )
        };
        let life = dummy.resources.get(&rules.life).copied().unwrap_or(0);
        let cap = rules
            .resource(&rules.life)
            .ok()
            .and_then(|max| dummy.stats.get(max));
        let mut line = format!(
            "{} {life}/{}",
            self.actor_name(DUMMY),
            cap.copied().unwrap_or(0)
        );
        if let Some(acting) = &hero.acting {
            let name = rules
                .ability(&acting.intent.ability)
                .map(|a| self.format(&a.name));
            line.push_str(&format!(
                "  |  {} in {}",
                name.unwrap_or_default(),
                seconds(acting.completes_at)
            ));
        }
        for (ability, ready) in &hero.cooldowns {
            if *ready > state.time
                && let Ok(definition) = rules.ability(ability)
            {
                let name = self.format(&definition.name);
                line.push_str(&format!("  |  {name} ready in {}", seconds(*ready)));
            }
        }
        line
    }
    /// Whether the rules follow where this actor is: the party, anyone asked to walk, and
    /// anyone a trigger watches.
    pub fn observed(&self, actor: ActorId) -> bool {
        self.driver.session().observed(actor)
    }
    /// The content's identity for a named area, when the content refers to that name.
    pub fn area(&self, name: &str) -> Option<AreaId> {
        let id = AreaId::try_from(name.to_owned()).ok()?;
        self.driver
            .session()
            .content()
            .game
            .world
            .areas
            .contains(&id)
            .then_some(id)
    }
    /// The areas the rules last heard the actor was inside.
    pub fn areas(&self, actor: ActorId) -> &BTreeSet<AreaId> {
        self.driver.state().areas(actor)
    }
    /// Tells the rules where an actor is, in metres. Nothing is sent while the actor stays
    /// in the same areas.
    pub fn observe(&mut self, actor: ActorId, position: [f64; 3], areas: BTreeSet<AreaId>) {
        if self.areas(actor) == &areas {
            return;
        }
        self.run(Command::World(WorldCommand::Observe {
            actor,
            position: millimetres(position),
            areas,
        }));
    }
    /// Records where actors stand, in metres, for a save. Which areas they are in stays as
    /// the rules last heard it.
    pub fn record(&mut self, positions: BTreeMap<ActorId, [f64; 3]>) {
        let positions = positions
            .into_iter()
            .map(|(actor, position)| (actor, millimetres(position)))
            .collect();
        self.run(Command::World(WorldCommand::Record { positions }));
    }
    /// Where the rules last recorded the actor, in metres.
    pub fn position(&self, actor: ActorId) -> Option<[f64; 3]> {
        let actor = self.driver.state().actor(actor).ok()?;
        Some(actor.position.millimetres.map(|mm| mm as f64 / 1000.))
    }
    /// Every walk the rules asked for and are waiting on.
    pub fn movements(&self) -> impl Iterator<Item = &Movement> {
        self.driver.state().world.movements.values()
    }
    /// The walk the rules asked of an actor, until it arrives or is given up.
    pub fn movement(&self, actor: ActorId) -> Option<&Movement> {
        self.driver.state().world.movements.get(&actor)
    }
    /// The engine cannot carry out the walk it was asked for.
    pub fn move_failed(&mut self, actor: ActorId, request: u64) {
        self.run(Command::World(WorldCommand::MoveFailed { actor, request }));
    }

    pub fn loads(&self) -> u64 {
        self.loads
    }
    pub fn quicksave(&mut self) {
        self.revision += 1;
        self.notice = match self.saves.quicksave(self.driver.session()) {
            Ok(_) => "Saved.".into(),
            Err(error) => format!("Save failed: {error}"),
        };
    }
    /// Replaces the playthrough with the quick slot; conversations in progress resume.
    pub fn quickload(&mut self) {
        let loaded = ContentRepository::open(&self.bundle)
            .map_err(|e| e.to_string())
            .and_then(|content| {
                let saved_with = self.saves.content_identity(SaveSlot::Quick);
                let changed = saved_with.is_ok_and(|saved| saved != content.identity());
                let session = self.saves.load(SaveSlot::Quick, content);
                session.map(|s| (s, changed)).map_err(|e| e.to_string())
            });
        self.revision += 1;
        self.notice = match loaded {
            Ok((session, changed)) => {
                self.driver.replace(session);
                self.loads += 1;
                self.ambient_line = None;
                if changed {
                    "Loaded. The content has changed since this save.".into()
                } else {
                    "Loaded.".into()
                }
            }
            Err(error) => format!("Load failed: {error}"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "yarra-story-test-{}",
                game_types::PlaythroughId::new()
            )))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn open(temp: &Temp) -> Story {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/gameplay/demo");
        Story::open(&source, &temp.0).unwrap()
    }
    /// Reads lines until choices appear, returning what was said.
    fn read(story: &mut Story) -> Vec<Panel> {
        let mut lines = Vec::new();
        for _ in 0..16 {
            match story.panel() {
                Some(panel) if panel.choices.is_empty() => {
                    lines.push(panel);
                    story.advance();
                }
                _ => break,
            }
        }
        lines
    }
    #[test]
    fn returning_the_key_unlocks_the_gate_and_a_reload_brings_the_lock_back() {
        let temp = Temp::new();
        let mut story = open(&temp);
        assert_eq!(story.guard_name(), "Gate guard");
        assert!(story.gate_locked());
        assert_eq!(
            story.summary(),
            "The old gate: active  |  Gate key: 1  |  Gate: locked"
        );
        story.quicksave();
        assert_eq!(story.notice, "Saved.");

        story.talk(None);
        let lines = read(&mut story);
        assert_eq!(lines[0].speaker, "Gate guard");
        assert_eq!(lines[0].text, "You found the gate key, Traveller!");
        // Mira travels with the player: she cuts in and the guard answers her.
        assert_eq!(lines[1].speaker, "Mira");
        assert_eq!(lines[2].speaker, "Gate guard");
        assert_eq!(lines[3].speaker, "Traveller");
        let choices = story.panel().unwrap().choices;
        assert_eq!(choices[0], "Here is your key.");
        assert_eq!(choices.len(), 3);
        story.choose(0);
        assert!(story.panel().is_none());
        assert!(!story.in_conversation());
        assert!(!story.gate_locked());
        assert_eq!(
            story.summary(),
            "The old gate: completed  |  Gate key: 0  |  Gate: unlocked"
        );

        story.quickload();
        assert_eq!(story.notice, "Loaded.");
        assert!(story.gate_locked());
        assert!(story.summary().contains("active  |  Gate key: 1"));
    }
    #[test]
    fn a_save_made_mid_conversation_resumes_at_the_same_line() {
        let temp = Temp::new();
        let mut story = open(&temp);
        story.talk(None);
        let first = story.panel().unwrap();
        story.advance();
        let second = story.panel().unwrap();
        assert_ne!(first, second);
        story.quicksave();
        read(&mut story);
        story.choose(2);
        assert!(story.panel().is_none());

        story.quickload();
        assert!(story.in_conversation());
        assert_eq!(story.panel().unwrap(), second);
        // Leaving is recorded as an interruption; the key stays with the player.
        story.leave();
        assert!(!story.in_conversation());
        assert!(story.gate_locked());
        story.talk(None);
        assert!(story.in_conversation());
    }
    #[test]
    fn input_that_does_not_apply_is_ignored() {
        let temp = Temp::new();
        let mut story = open(&temp);
        story.advance();
        story.choose(0);
        story.leave();
        assert!(story.panel().is_none());
        story.quickload();
        assert!(story.notice.starts_with("Load failed"));
        story.talk(None);
        // A choice cannot be picked while a line is still waiting to be read.
        let line = story.panel().unwrap();
        story.choose(0);
        assert_eq!(story.panel().unwrap(), line);
        read(&mut story);
        story.choose(9);
        assert!(story.in_conversation());
    }
    const APPROACH: AreaId = AreaId::named("guard/approach");
    const GATE_POST: AreaId = AreaId::named("guard/gate_post");
    /// Lets ambient lines play until the one being spoken changes.
    fn hear_next(story: &mut Story) -> Option<Bark> {
        let spoken = story.bark();
        for _ in 0..20 {
            story.tick(0.5);
            if story.bark() != spoken {
                break;
            }
        }
        story.bark()
    }
    #[test]
    fn walking_up_starts_the_banter_and_sends_the_guard_to_open_the_gate() {
        let temp = Temp::new();
        let mut story = open(&temp);
        assert!(story.observed(HERO) && story.observed(MIRA) && !story.observed(GUARD));
        assert_eq!(story.area("guard/approach"), Some(APPROACH));
        assert_eq!(story.area("somewhere/else"), None);
        // Standing outside every area is nothing to report.
        let revision = story.revision();
        story.observe(HERO, [0., 0., 0.], BTreeSet::new());
        assert_eq!(story.revision(), revision);

        story.observe(HERO, [3., 0.5, -2.], [APPROACH].into());
        assert!(story.summary().ends_with("|  In: guard/approach"));
        // Mira remarks on it while the player keeps control.
        assert!(!story.in_conversation());
        let remark = story.bark().unwrap();
        assert_eq!(remark.speaker, "Mira");
        let reply = hear_next(&mut story).unwrap();
        assert_eq!(reply.speaker, "Traveller");
        assert_eq!(hear_next(&mut story), None);

        // The guard was asked to the gate, so the rules follow him until he gets there.
        assert_eq!(story.movement(GUARD).map(|m| m.to), Some(GATE_POST));
        assert!(story.observed(GUARD) && story.gate_locked());
        story.observe(GUARD, [9.5, 0.5, -3.], [APPROACH, GATE_POST].into());
        assert!(story.movement(GUARD).is_none() && !story.observed(GUARD));
        assert!(story.areas(GUARD).contains(&GATE_POST));
        assert!(!story.gate_locked());
        assert!(
            story
                .summary()
                .starts_with("The old gate: completed  |  Gate key: 0")
        );
        assert_eq!(story.notice, "");
    }
    #[test]
    fn a_walk_runs_out_of_time_but_not_while_a_conversation_holds_the_world() {
        let temp = Temp::new();
        let mut story = open(&temp);
        story.observe(HERO, [3., 0., 0.], [APPROACH].into());
        let request = story.movement(GUARD).unwrap().request;
        story.talk(None);
        assert!(story.in_conversation());
        for _ in 0..30 {
            story.tick(1.0);
        }
        assert!(story.movement(GUARD).is_some());
        // The banter waited too.
        assert_eq!(story.bark().unwrap().speaker, "Mira");
        story.leave();
        for _ in 0..9 {
            story.tick(1.0);
        }
        assert!(story.movement(GUARD).is_some());
        story.tick(1.0);
        story.tick(1.0);
        assert!(story.movement(GUARD).is_none());
        assert!(story.gate_locked());
        // A report about the walk that is over is refused and changes nothing.
        story.move_failed(GUARD, request);
        assert!(
            story.notice.contains("no longer requested"),
            "{}",
            story.notice
        );
    }
    #[test]
    fn a_load_brings_back_the_spoken_line_the_walk_and_where_everyone_stood() {
        let temp = Temp::new();
        let mut story = open(&temp);
        story.observe(HERO, [3.25, 0.5, -2.], [APPROACH].into());
        let remark = story.bark().unwrap();
        story.observe(GUARD, [6., 0.25, -2.6], [APPROACH].into());
        story.quicksave();
        assert_eq!(hear_next(&mut story).unwrap().speaker, "Traveller");
        story.observe(HERO, [40., 0., 0.], BTreeSet::new());
        story.observe(GUARD, [9.5, 0.5, -3.], [GATE_POST].into());
        assert!(!story.gate_locked());

        story.quickload();
        assert_eq!((story.notice.as_str(), story.loads()), ("Loaded.", 1));
        assert_eq!(story.bark(), Some(remark));
        assert_eq!(story.position(HERO), Some([3.25, 0.5, -2.]));
        assert_eq!(story.position(GUARD), Some([6., 0.25, -2.6]));
        assert!(story.areas(HERO).contains(&APPROACH));
        assert_eq!(story.movement(GUARD).map(|m| m.to), Some(GATE_POST));
        assert!(story.gate_locked());
    }
    #[test]
    fn the_reward_is_a_level_the_guard_gives_lessons_and_the_dummy_takes_blows() {
        let temp = Temp::new();
        let mut story = open(&temp);
        assert!(story.character().starts_with("Traveller: level 1  |  0 XP"));
        assert!(story.character().contains("Health 50/100"));
        assert_eq!(
            story.topic().map(|(_, label)| label).as_deref(),
            Some("About the gate key")
        );
        // The escort: into the approach, the guard walks to the gate and takes the key.
        story.observe(HERO, [3., 0., 0.], [APPROACH].into());
        story.observe(GUARD, [9.5, 0., -3.], [APPROACH, GATE_POST].into());
        assert!(!story.gate_locked());
        let sheet = story.character();
        assert!(
            sheet.starts_with("Traveller: level 2  |  120 XP"),
            "{sheet}"
        );
        assert!(
            sheet.contains("Health 50/110") && sheet.contains("Gold 100"),
            "{sheet}"
        );
        assert_eq!(story.points(), (2, 3));

        // Now the guard has lessons to offer.
        let (topic, label) = story.topic().unwrap();
        assert_eq!(label, "About sword lessons");
        story.talk(Some(topic));
        assert!(
            story
                .panel()
                .unwrap()
                .text
                .starts_with("You opened my gate.")
        );
        story.advance();
        assert_eq!(
            story.panel().unwrap().choices,
            ["Teach me. (20 gold)", "Goodbye."]
        );
        story.choose(0);
        story.advance();
        story.choose(0);
        story.advance();
        // Two ranks is as far as an adventurer is taught: only the farewell is left.
        assert_eq!(story.panel().unwrap().choices, ["Goodbye."]);
        story.choose(0);
        assert!(story.panel().is_none() && !story.in_conversation());
        let sheet = story.character();
        assert!(
            sheet.contains("Attack 14") && sheet.contains("Swordsmanship 2"),
            "{sheet}"
        );
        assert!(sheet.contains("Gold 60"), "{sheet}");
        story.spend("strength");
        assert!(story.character().contains("Attack 15"));
        assert_eq!(story.points(), (1, 0));
        story.drink();
        assert!(story.character().contains("Health 75/110"));

        // Sparring: a strike takes a second and a half, and goes on until told to stop.
        assert_eq!(story.fight(), "Training dummy 200/200");
        story.strike();
        assert!(story.busy() && story.fight().contains("Strike in 1.5 s"));
        for _ in 0..16 {
            story.tick(0.1);
        }
        let after_one = story.fight();
        assert!(
            !after_one.starts_with("Training dummy 200/200"),
            "{after_one}"
        );
        story.power_strike();
        for _ in 0..30 {
            story.tick(0.1);
        }
        assert!(
            story.fight().contains("Power strike ready in"),
            "{}",
            story.fight()
        );
        assert!(
            story.character().contains("Stamina 10/20"),
            "{}",
            story.character()
        );
        story.stop();
        assert!(!story.busy());
        assert_eq!(story.notice, "");
    }
}
