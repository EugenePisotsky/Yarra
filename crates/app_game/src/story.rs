//! First playable gameplay slice, opt-in with `--story DIR`: a guard and a gate near the
//! start, a conversation, triggers that react to where the party walks, and quick save/load.
//! The rules run in `gameplay`; this module only publishes the authored project, forwards
//! input and what the engine observes as commands, and shows read models.
use game_content::{ContentRepository, LoadedProject, RuntimeSession};
use game_types::{ActorId, AreaId, BoundText, ItemDefinitionId, Key, ObjectId, QuestId, TextRef};
use gameplay::actors::Position;
use gameplay::dialogue::{Mode, RunStatus};
use gameplay::{
    Command, ConversationKey, ConversationView, GameEvent, GameSession, Movement, WorldCommand,
    WorldEvent, quests,
};
use localization::Arguments;
use save::{SaveDirectory, SaveSlot};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

mod scene;
pub(crate) use scene::install;

/// Starting conditions for the slice, relative to the project directory.
const SCENARIO: &str = "scenarios/island.ron";
const HERO: ActorId = ActorId::named("hero");
const GUARD: ActorId = ActorId::named("guard");
const MIRA: ActorId = ActorId::named("mira");
const GATE: ObjectId = ObjectId::named("guard/old_gate");
const GATE_QUEST: QuestId = QuestId::named("guard/gate");
const GATE_KEY: ItemDefinitionId = ItemDefinitionId::named("old_gate_key");
/// Queued work carried out after one command; anything beyond it waits for the next.
const PUMP_LIMIT: usize = 64;
/// Game time moves in steps of this many milliseconds.
const TIME_STEP_MS: u64 = 100;

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
    session: RuntimeSession,
    bundle: PathBuf,
    saves: SaveDirectory,
    locale: String,
    /// The blocking conversation on screen, and others that started meanwhile.
    conversation: Option<ConversationKey>,
    waiting: VecDeque<ConversationKey>,
    /// The ambient conversation being spoken, and others waiting their turn.
    ambient: Option<ConversationKey>,
    ambient_waiting: VecDeque<ConversationKey>,
    /// Seconds the ambient line has been up, and how long it stays once that is worked out.
    ambient_shown: f32,
    ambient_duration: Option<f32>,
    /// Real time not yet turned into game time.
    unspent_seconds: f32,
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
        let mut story = Self {
            project,
            session,
            bundle,
            saves,
            locale,
            conversation: None,
            waiting: VecDeque::new(),
            ambient: None,
            ambient_waiting: VecDeque::new(),
            ambient_shown: 0.,
            ambient_duration: None,
            unspent_seconds: 0.,
            loads: 0,
            guard_name: String::new(),
            revision: 0,
            notice: String::new(),
        };
        story.guard_name = story.actor_name(GUARD);
        story.resume();
        story.pump();
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
        let state = self.session.state();
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
        self.conversation.is_some()
    }
    pub fn gate_locked(&self) -> bool {
        self.session
            .state()
            .object(self.session.content(), GATE)
            .is_ok_and(|gate| gate.locked)
    }
    /// One line for the corner of the screen: the quest, what the player carries for it and
    /// the named areas the player stands in.
    pub fn summary(&self) -> String {
        let state = self.session.state();
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

    /// Applies a command, then carries out whatever the rules queued because of it.
    fn run(&mut self, command: Command) -> bool {
        self.revision += 1;
        let accepted = self.apply(command);
        self.pump();
        accepted
    }
    fn apply(&mut self, command: Command) -> bool {
        match self.session.apply(command) {
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
            GameEvent::DialogueStarted { key, mode } => self.enqueue(key, mode),
            GameEvent::World(WorldEvent::TriggerFailed { trigger, reason }) => {
                self.notice = format!("Trigger {trigger} failed: {reason}");
            }
            GameEvent::World(WorldEvent::DialogueRefused { dialogue, reason }) => {
                self.notice = format!("Conversation {dialogue} did not start: {reason}");
            }
            _ => {}
        }
    }
    /// Triggers reacting to what happened, conversations they start and walks that ran out
    /// of time.
    fn pump(&mut self) {
        for _ in 0..PUMP_LIMIT {
            if !self.session.world_work_pending() {
                break;
            }
            self.revision += 1;
            if !self.apply(Command::World(WorldCommand::ProcessNext)) {
                break;
            }
        }
    }
    fn enqueue(&mut self, key: ConversationKey, mode: Mode) {
        let (current, waiting) = match mode {
            Mode::Blocking => (&mut self.conversation, &mut self.waiting),
            Mode::Ambient => (&mut self.ambient, &mut self.ambient_waiting),
        };
        if current.is_none() {
            *current = Some(key);
        } else if *current != Some(key) && !waiting.contains(&key) {
            waiting.push_back(key);
        }
    }
    /// Picks up the conversations a new or loaded playthrough is in the middle of.
    fn resume(&mut self) {
        self.conversation = None;
        self.waiting.clear();
        self.ambient = None;
        self.ambient_waiting.clear();
        self.ambient_shown = 0.;
        self.ambient_duration = None;
        self.unspent_seconds = 0.;
        let content = self.session.content();
        let active: Vec<(ConversationKey, Mode)> = self
            .session
            .state()
            .conversations
            .iter()
            .filter(|(_, run)| run.status == RunStatus::Active)
            .filter_map(|(key, _)| Some((*key, content.dialogue_contract(key.dialogue).ok()?.mode)))
            .collect();
        for (key, mode) in active {
            self.enqueue(key, mode);
        }
    }
    /// The conversation of one kind being shown, moving on to the next waiting one when it
    /// has ended.
    fn current(&mut self, mode: Mode) -> Option<ConversationView> {
        loop {
            let key = match mode {
                Mode::Blocking => self.conversation,
                Mode::Ambient => self.ambient,
            }?;
            match self.session.conversation_view(key) {
                Ok(view) if view.status == RunStatus::Active => return Some(view),
                Ok(_) => {}
                Err(error) => self.notice = error.to_string(),
            }
            match mode {
                Mode::Blocking => self.conversation = self.waiting.pop_front(),
                Mode::Ambient => {
                    self.ambient = self.ambient_waiting.pop_front();
                    self.ambient_shown = 0.;
                    self.ambient_duration = None;
                }
            }
        }
    }

    /// Starts or resumes the conversation the guard's entry rules select.
    pub fn talk(&mut self) {
        if self.conversation.is_some() {
            return;
        }
        let accepted = self.run(Command::Talk {
            participant: HERO,
            speaker: GUARD,
            topic: None,
            bindings: Default::default(),
        });
        if accepted {
            self.notice.clear();
            // A conversation that was already under way announces nothing; pick it up.
            if self.conversation.is_none() {
                self.conversation = self.active_conversation();
            }
        }
    }
    fn active_conversation(&self) -> Option<ConversationKey> {
        let state = self.session.state();
        let selection = state.selection(gameplay::dialogue::InteractionKey {
            participant: HERO,
            speaker: GUARD,
        })?;
        let key = ConversationKey {
            dialogue: selection.dialogue,
            participant: HERO,
            speaker: GUARD,
        };
        state
            .conversation(key)
            .is_ok_and(|c| c.status == RunStatus::Active)
            .then_some(key)
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
        let Some(key) = self.conversation else {
            return;
        };
        if let Ok(view) = self.session.conversation_view(key)
            && view.line.is_some()
        {
            self.run(Command::AdvanceLine {
                key,
                expected: view.token,
            });
        }
    }
    /// Pick the choice at this position in the list the panel showed.
    pub fn choose(&mut self, index: usize) {
        let Some(key) = self.conversation else {
            return;
        };
        let Ok(view) = self.session.conversation_view(key) else {
            return;
        };
        if view.line.is_some() {
            return;
        }
        if let Some(choice) = view.choices.get(index) {
            let choice: Key = choice.id.clone();
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
        let Some(key) = self.conversation else {
            return;
        };
        if let Ok(view) = self.session.conversation_view(key)
            && view.status == RunStatus::Active
        {
            self.run(Command::InterruptDialogue {
                key,
                expected: view.token,
            });
        }
        self.revision += 1;
        self.current(Mode::Blocking);
    }

    /// Lets `seconds` of play pass: game time moves on, a walk can run out of time and an
    /// ambient line gives way to the next. A blocking conversation pauses all of it.
    pub fn tick(&mut self, seconds: f32) {
        if self.current(Mode::Blocking).is_some() {
            return;
        }
        // A long hitch does not fast-forward the world.
        let seconds = seconds.clamp(0., 1.);
        self.unspent_seconds += seconds;
        let steps = (self.unspent_seconds * 1000.) as u64 / TIME_STEP_MS;
        if steps > 0 {
            let millis = steps * TIME_STEP_MS;
            self.unspent_seconds -= millis as f32 / 1000.;
            match self.session.apply(Command::AdvanceTime { millis }) {
                Ok(outcome) => {
                    if !outcome.events.is_empty() {
                        self.revision += 1;
                    }
                    for event in outcome.events {
                        self.note(event);
                    }
                }
                Err(error) => self.notice = error.to_string(),
            }
            self.pump();
        }
        let Some(view) = self.current(Mode::Ambient) else {
            return;
        };
        self.ambient_shown += seconds;
        let duration = match self.ambient_duration {
            Some(duration) => duration,
            None => {
                // Long enough to read: a moment, plus time for each character.
                let length = view
                    .line
                    .as_ref()
                    .map_or(0, |line| self.bound(&line.text).chars().count());
                *self
                    .ambient_duration
                    .insert((1.5 + 0.06 * length as f32).min(8.))
            }
        };
        if self.ambient_shown >= duration {
            self.ambient_shown = 0.;
            self.ambient_duration = None;
            self.run(Command::AdvanceLine {
                key: view.key,
                expected: view.token,
            });
        }
    }

    /// Whether the rules follow where this actor is: the party, and anyone asked to walk.
    pub fn tracked(&self, actor: ActorId) -> bool {
        let state = self.session.state();
        state.party.contains(&actor) || state.world.movements.contains_key(&actor)
    }
    /// The content's identity for a named area, when the content refers to that name.
    pub fn area(&self, name: &str) -> Option<AreaId> {
        let id = AreaId::try_from(name.to_owned()).ok()?;
        self.session
            .content()
            .game
            .world
            .areas
            .contains(&id)
            .then_some(id)
    }
    /// The areas the rules last heard the actor was inside.
    pub fn areas(&self, actor: ActorId) -> &BTreeSet<AreaId> {
        self.session.state().areas(actor)
    }
    /// Tells the rules where an actor is, in metres. Nothing is sent while the actor stays
    /// in the same areas, unless the position is to be `record`ed for a save.
    pub fn observe(
        &mut self,
        actor: ActorId,
        position: [f64; 3],
        areas: BTreeSet<AreaId>,
        record: bool,
    ) {
        if !record && self.areas(actor) == &areas {
            return;
        }
        let position = Position {
            millimetres: position.map(|metres| (metres * 1000.).round() as i64),
        };
        self.run(Command::World(WorldCommand::Observe {
            actor,
            position,
            areas,
        }));
    }
    /// Where the rules last recorded the actor, in metres.
    pub fn position(&self, actor: ActorId) -> Option<[f64; 3]> {
        let actor = self.session.state().actor(actor).ok()?;
        Some(actor.position.millimetres.map(|mm| mm as f64 / 1000.))
    }
    /// The walk the rules asked of an actor, until it arrives or is given up.
    pub fn movement(&self, actor: ActorId) -> Option<&Movement> {
        self.session.state().world.movements.get(&actor)
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
        self.notice = match self.saves.quicksave(&self.session) {
            Ok(_) => "Saved.".into(),
            Err(error) => format!("Save failed: {error}"),
        };
    }
    /// Replaces the playthrough with the quick slot; conversations in progress resume.
    pub fn quickload(&mut self) {
        let loaded = ContentRepository::open(&self.bundle)
            .map_err(|e| e.to_string())
            .and_then(|content| {
                self.saves
                    .load(SaveSlot::Quick, content)
                    .map_err(|e| e.to_string())
            });
        self.revision += 1;
        self.notice = match loaded {
            Ok(session) => {
                self.session = session;
                self.loads += 1;
                self.resume();
                "Loaded.".into()
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

        story.talk();
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
        story.talk();
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
        story.talk();
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
        story.talk();
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
        assert!(story.tracked(HERO) && story.tracked(MIRA) && !story.tracked(GUARD));
        assert_eq!(story.area("guard/approach"), Some(APPROACH));
        assert_eq!(story.area("somewhere/else"), None);
        // Standing outside every area is nothing to report.
        let revision = story.revision();
        story.observe(HERO, [0., 0., 0.], BTreeSet::new(), false);
        assert_eq!(story.revision(), revision);

        story.observe(HERO, [3., 0.5, -2.], [APPROACH].into(), false);
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
        assert!(story.tracked(GUARD) && story.gate_locked());
        story.observe(GUARD, [9.5, 0.5, -3.], [APPROACH, GATE_POST].into(), false);
        assert!(story.movement(GUARD).is_none() && !story.tracked(GUARD));
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
        story.observe(HERO, [3., 0., 0.], [APPROACH].into(), false);
        let request = story.movement(GUARD).unwrap().request;
        story.talk();
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
        story.observe(HERO, [3.25, 0.5, -2.], [APPROACH].into(), false);
        let remark = story.bark().unwrap();
        story.observe(GUARD, [6., 0.25, -2.6], [APPROACH].into(), true);
        story.quicksave();
        assert_eq!(hear_next(&mut story).unwrap().speaker, "Traveller");
        story.observe(HERO, [40., 0., 0.], BTreeSet::new(), false);
        story.observe(GUARD, [9.5, 0.5, -3.], [GATE_POST].into(), false);
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
}
