//! First playable gameplay slice, opt-in with `--story DIR`: a guard and a gate near the
//! start, a conversation, and quick save/load. The rules run in `gameplay`; this module only
//! publishes the authored project, forwards input as commands and shows read models.
use game_content::{ContentRepository, LoadedProject, RuntimeSession};
use game_types::{ActorId, ItemDefinitionId, Key, ObjectId, QuestId, TextRef};
use gameplay::dialogue::RunStatus;
use gameplay::{Command, ConversationKey, GameSession, quests};
use localization::Arguments;
use save::{SaveDirectory, SaveSlot};
use std::path::{Path, PathBuf};

mod scene;
pub(crate) use scene::install;

/// Starting conditions for the slice, relative to the project directory.
const SCENARIO: &str = "scenarios/island.ron";
const HERO: ActorId = ActorId::named("hero");
const GUARD: ActorId = ActorId::named("guard");
const GATE: ObjectId = ObjectId::named("guard/old_gate");
const GATE_QUEST: QuestId = QuestId::named("guard/gate");
const GATE_KEY: ItemDefinitionId = ItemDefinitionId::named("old_gate_key");

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

pub(crate) struct Story {
    project: LoadedProject,
    session: RuntimeSession,
    bundle: PathBuf,
    saves: SaveDirectory,
    locale: String,
    conversation: Option<ConversationKey>,
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
    pub fn in_conversation(&self) -> bool {
        self.conversation.is_some()
    }
    pub fn gate_locked(&self) -> bool {
        self.session
            .state()
            .object(self.session.content(), GATE)
            .is_ok_and(|gate| gate.locked)
    }
    /// One line for the corner of the screen: the quest and what the player carries for it.
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
        format!("{title}: {status}  |  Gate key: {keys}  |  Gate: {gate}")
    }
    fn run(&mut self, command: Command) -> bool {
        self.revision += 1;
        match self.session.apply(command) {
            Ok(_) => true,
            Err(error) => {
                self.notice = error.to_string();
                false
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
            self.conversation = self.active_conversation();
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
        let key = self.conversation?;
        let view = match self.session.conversation_view(key) {
            Ok(view) if view.status == RunStatus::Active => view,
            Ok(_) => {
                self.conversation = None;
                return None;
            }
            Err(error) => {
                self.notice = error.to_string();
                self.conversation = None;
                return None;
            }
        };
        let localization = self.project.localization();
        let bound = |text| match localization.format_bound(&self.locale, text) {
            Ok(text) => plain(text.value),
            Err(error) => format!("<{error}>"),
        };
        Some(match &view.line {
            Some(line) => Panel {
                speaker: self.actor_name(line.speaker),
                text: bound(&line.text),
                choices: vec![],
            },
            None => Panel {
                speaker: self.actor_name(HERO),
                text: String::new(),
                choices: view.choices.iter().map(|c| bound(&c.text)).collect(),
            },
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
        let Some(key) = self.conversation.take() else {
            return;
        };
        self.revision += 1;
        if let Ok(view) = self.session.conversation_view(key)
            && view.status == RunStatus::Active
        {
            self.run(Command::InterruptDialogue {
                key,
                expected: view.token,
            });
        }
    }
    pub fn quicksave(&mut self) {
        self.revision += 1;
        self.notice = match self.saves.quicksave(&self.session) {
            Ok(_) => "Saved.".into(),
            Err(error) => format!("Save failed: {error}"),
        };
    }
    /// Replaces the playthrough with the quick slot; a conversation in progress resumes.
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
                self.conversation = self.active_conversation();
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
}
