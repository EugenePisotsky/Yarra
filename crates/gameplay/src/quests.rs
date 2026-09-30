//! Quest progression contracts. No dialogue, inventory, SQLite or presentation services.
use game_types::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub id: Key,
    pub title: TextRef,
    pub required: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quest {
    pub id: QuestId,
    pub title: TextRef,
    pub objectives: Vec<Objective>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    NotStarted,
    Active,
    Completed,
    Failed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Progress {
    pub quest: QuestId,
    pub status: Status,
    pub completed: BTreeSet<Key>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transition {
    Start,
    CompleteObjective(Key),
    Complete,
    Fail,
}
impl Quest {
    pub fn validate(&self) -> Result<()> {
        self.title.validate()?;
        require(!self.objectives.is_empty(), "a quest has objectives")?;
        let mut ids = BTreeSet::new();
        for objective in &self.objectives {
            require(ids.insert(&objective.id), "duplicate quest objective")?;
            objective.title.validate()?;
        }
        Ok(())
    }
    pub fn objective(&self, id: &Key) -> Result<&Objective> {
        self.objectives
            .iter()
            .find(|o| &o.id == id)
            .ok_or_else(|| Invalid("unknown quest objective".into()))
    }
}
impl Progress {
    pub fn new(quest: QuestId) -> Self {
        Self {
            quest,
            status: Status::NotStarted,
            completed: BTreeSet::new(),
        }
    }
    pub fn validate(&self, definition: &Quest) -> Result<()> {
        require(
            self.quest == definition.id,
            "quest progress identity mismatch",
        )?;
        for id in &self.completed {
            definition.objective(id)?;
        }
        require(
            self.status != Status::NotStarted || self.completed.is_empty(),
            "unstarted quest has completed objectives",
        )?;
        require(
            self.status != Status::Completed
                || definition
                    .objectives
                    .iter()
                    .filter(|o| o.required)
                    .all(|o| self.completed.contains(&o.id)),
            "quest has incomplete required objectives",
        )
    }
    /// Terminal progress cannot restart. Duplicate objective completion is rejected explicitly.
    pub fn apply(&mut self, definition: &Quest, transition: &Transition) -> Result<()> {
        self.validate(definition)?;
        match transition {
            Transition::Start => {
                require(
                    self.status == Status::NotStarted,
                    "quest has already started",
                )?;
                self.status = Status::Active;
            }
            Transition::CompleteObjective(id) => {
                require(self.status == Status::Active, "quest is not active")?;
                definition.objective(id)?;
                require(!self.completed.contains(id), "objective already completed")?;
                self.completed.insert(id.clone());
            }
            Transition::Complete => {
                require(self.status == Status::Active, "quest is not active")?;
                require(
                    definition
                        .objectives
                        .iter()
                        .filter(|o| o.required)
                        .all(|o| self.completed.contains(&o.id)),
                    "quest has incomplete required objectives",
                )?;
                self.status = Status::Completed;
            }
            Transition::Fail => {
                require(self.status == Status::Active, "quest is not active")?;
                self.status = Status::Failed;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(s: &str) -> Key {
        Key::new(s).unwrap()
    }
    fn quest() -> Quest {
        Quest {
            id: QuestId([1; 16]),
            title: TextRef::from("Find the key"),
            objectives: vec![
                Objective {
                    id: key("key"),
                    title: TextRef::from("Return the key"),
                    required: true,
                },
                Objective {
                    id: key("secret"),
                    title: TextRef::from("Find the secret"),
                    required: false,
                },
            ],
        }
    }
    #[test]
    fn required_objectives_gate_completion_and_optional_objectives_do_not() {
        let q = quest();
        let mut p = Progress::new(q.id);
        p.apply(&q, &Transition::Start).unwrap();
        let before = p.clone();
        assert!(p.apply(&q, &Transition::Complete).is_err());
        assert_eq!(p, before);
        p.apply(&q, &Transition::CompleteObjective(key("key")))
            .unwrap();
        p.apply(&q, &Transition::Complete).unwrap();
        assert_eq!(p.status, Status::Completed);
        assert!(!p.completed.contains(&key("secret")));
        p.validate(&q).unwrap();
    }
    #[test]
    fn invalid_and_terminal_transitions_never_mutate_progress() {
        let q = quest();
        let mut p = Progress::new(q.id);
        for action in [
            Transition::Complete,
            Transition::Fail,
            Transition::CompleteObjective(key("key")),
        ] {
            let before = p.clone();
            assert!(p.apply(&q, &action).is_err());
            assert_eq!(p, before);
        }
        p.apply(&q, &Transition::Start).unwrap();
        p.apply(&q, &Transition::CompleteObjective(key("key")))
            .unwrap();
        for action in [
            Transition::Start,
            Transition::CompleteObjective(key("missing")),
            Transition::CompleteObjective(key("key")),
        ] {
            let before = p.clone();
            assert!(p.apply(&q, &action).is_err());
            assert_eq!(p, before);
        }
        p.apply(&q, &Transition::Fail).unwrap();
        let before = p.clone();
        assert!(p.apply(&q, &Transition::Start).is_err());
        assert!(
            p.apply(&q, &Transition::CompleteObjective(key("secret")))
                .is_err()
        );
        assert_eq!(p, before);
    }
    #[test]
    fn invalid_definitions_and_saved_progress_are_rejected() {
        let mut q = quest();
        q.objectives.push(q.objectives[0].clone());
        assert!(q.validate().is_err());
        let q = quest();
        let mut p = Progress::new(q.id);
        p.status = Status::Completed;
        assert!(p.validate(&q).is_err());
        p.status = Status::Active;
        p.completed.insert(key("missing"));
        assert!(p.validate(&q).is_err());
        p.completed.clear();
        p.quest = QuestId([2; 16]);
        assert!(p.validate(&q).is_err());
    }
}
