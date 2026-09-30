use crate::WorkingStore;
use crate::{Result, storage};
use game_types::{GameTime, PlaythroughId, require};
use gameplay::{ContentIdentity, ContentSource, GameSession};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SaveSlot {
    Manual(u16),
    Quick,
    Auto(u16),
}
impl SaveSlot {
    fn filename(self) -> String {
        match self {
            Self::Manual(id) => format!("manual-{id}.sqlite"),
            Self::Quick => "quick.sqlite".into(),
            Self::Auto(id) => format!("auto-{id}.sqlite"),
        }
    }
    fn parse(filename: &str) -> Option<Self> {
        if filename == "quick.sqlite" {
            return Some(Self::Quick);
        }
        let (kind, number) = filename.strip_suffix(".sqlite")?.split_once('-')?;
        let slot = match kind {
            "manual" => Self::Manual(number.parse().ok()?),
            "auto" => Self::Auto(number.parse().ok()?),
            _ => return None,
        };
        (slot.filename() == filename).then_some(slot)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveInfo {
    pub slot: SaveSlot,
    pub title: String,
    pub playthrough: PlaythroughId,
    pub generation: u64,
    pub game_time: GameTime,
    pub saved_at_ms: u64,
    /// Monotonic within a slot; rotating autosaves share one sequence.
    pub sequence: u64,
}
pub struct SaveDirectory {
    root: PathBuf,
    autosave_slots: u16,
}
impl SaveDirectory {
    pub fn new(root: impl AsRef<Path>, autosave_slots: u16) -> Result<Self> {
        require(
            (1..=32).contains(&autosave_slots),
            "autosave retention must be 1..32",
        )?;
        std::fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().into(),
            autosave_slots,
        })
    }
    pub fn path(&self, slot: SaveSlot) -> PathBuf {
        self.root.join(slot.filename())
    }
    pub fn list(&self) -> Result<Vec<SaveInfo>> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let Some(slot) = SaveSlot::parse(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            let info = storage::info(&entry.path())?;
            require(info.slot == slot, "slot filename differs from metadata")?;
            result.push(info);
        }
        result.sort_by_key(|i| i.slot);
        Ok(result)
    }
    pub fn save<C: ContentSource>(
        &self,
        slot: SaveSlot,
        session: &GameSession<WorkingStore, C>,
        title: impl Into<String>,
    ) -> Result<SaveInfo> {
        if let SaveSlot::Auto(index) = slot {
            require(
                index < self.autosave_slots,
                "autosave index exceeds retention",
            )?;
        }
        let title = title.into();
        require(
            !title.trim().is_empty() && title.len() <= 256,
            "invalid save title",
        )?;
        let saved_at_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| game_types::Invalid("clock predates Unix epoch".into()))?
            .as_millis();
        let saved_at_ms = u64::try_from(saved_at_ms)
            .map_err(|_| game_types::Invalid("wall clock overflow".into()))?;
        let header = session.header()?;
        let info = SaveInfo {
            slot,
            title,
            playthrough: header.playthrough,
            generation: header.generation,
            game_time: header.time,
            saved_at_ms,
            sequence: self.next_sequence(slot)?,
        };
        storage::write(&self.path(slot), session, &info)?;
        Ok(info)
    }
    pub fn quicksave<C: ContentSource>(
        &self,
        session: &GameSession<WorkingStore, C>,
    ) -> Result<SaveInfo> {
        self.save(SaveSlot::Quick, session, "Quicksave")
    }
    /// Rotate only autosaves. Manual and quick slots never participate in retention.
    pub fn autosave<C: ContentSource>(
        &self,
        session: &GameSession<WorkingStore, C>,
    ) -> Result<SaveInfo> {
        let mut oldest = None;
        for index in 0..self.autosave_slots {
            let slot = SaveSlot::Auto(index);
            let path = self.path(slot);
            if !path.try_exists()? {
                return self.save(slot, session, "Autosave");
            }
            let info = storage::info(&path)?;
            require(info.slot == slot, "slot metadata mismatch")?;
            if oldest.is_none_or(|(sequence, _)| info.sequence < sequence) {
                oldest = Some((info.sequence, slot));
            }
        }
        self.save(
            oldest.expect("nonzero autosave count").1,
            session,
            "Autosave",
        )
    }
    /// Identify the retained mechanical publication needed to restore this slot.
    pub fn content_identity(&self, slot: SaveSlot) -> Result<ContentIdentity> {
        storage::identity(&self.path(slot))
    }
    /// Metadata checked immediately; individual records/content are validated on bounded access.
    pub fn load<C: ContentSource>(
        &self,
        slot: SaveSlot,
        content: C,
        working_path: impl AsRef<Path>,
    ) -> Result<GameSession<WorkingStore, C>> {
        storage::load(&self.path(slot), slot, content, working_path.as_ref())
    }
    fn next_sequence(&self, slot: SaveSlot) -> Result<u64> {
        let slots: Vec<_> = if matches!(slot, SaveSlot::Auto(_)) {
            (0..self.autosave_slots).map(SaveSlot::Auto).collect()
        } else {
            vec![slot]
        };
        let mut maximum = 0;
        for slot in slots {
            let path = self.path(slot);
            if path.try_exists()? {
                let info = storage::info(&path)?;
                require(info.slot == slot, "slot metadata mismatch")?;
                maximum = maximum.max(info.sequence);
            }
        }
        maximum
            .checked_add(1)
            .ok_or_else(|| game_types::Invalid("save sequence overflow".into()).into())
    }
}
