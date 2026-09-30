use crate::{Result, SaveError};
use game_types::{GameTime, OwnerId, PlaythroughId, require};
use gameplay::{ContentIdentity, ContentSource, GameSession, SessionState};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Bumped whenever saved state changes shape. Older saves are rejected, not migrated.
pub const SAVE_FORMAT: u32 = 12;
const MAX_HEADER_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SaveSlot {
    Manual(u16),
    Quick,
    Auto(u16),
}
impl SaveSlot {
    fn filename(self) -> String {
        match self {
            Self::Manual(id) => format!("manual-{id}.save"),
            Self::Quick => "quick.save".into(),
            Self::Auto(id) => format!("auto-{id}.save"),
        }
    }
    fn parse(filename: &str) -> Option<Self> {
        if filename == "quick.save" {
            return Some(Self::Quick);
        }
        let (kind, number) = filename.strip_suffix(".save")?.split_once('-')?;
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
/// First line of a save file. Listing slots reads only this line, never the state.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    yarra_save: u32,
    info: SaveInfo,
    content: ContentIdentity,
}
fn read_header(path: &Path) -> Result<(Header, BufReader<fs::File>)> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    let mut line = Vec::new();
    std::io::Read::take(&mut reader, MAX_HEADER_BYTES).read_until(b'\n', &mut line)?;
    #[derive(Deserialize)]
    struct Probe {
        yarra_save: u32,
    }
    let probe: Probe = serde_json::from_slice(&line).map_err(|_| SaveError::NotASave)?;
    if probe.yarra_save != SAVE_FORMAT {
        return Err(SaveError::Format(probe.yarra_save));
    }
    Ok((serde_json::from_slice(&line)?, reader))
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
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().into(),
            autosave_slots,
        })
    }
    pub fn path(&self, slot: SaveSlot) -> PathBuf {
        self.root.join(slot.filename())
    }
    /// What a slot holds; `None` when it is empty or holds nothing this build can load: an
    /// older format, a foreign or damaged file. Saving treats such a slot as empty, and
    /// loading it still says what is wrong.
    fn info(&self, slot: SaveSlot) -> Result<Option<SaveInfo>> {
        let path = self.path(slot);
        if !path.try_exists()? {
            return Ok(None);
        }
        match read_header(&path) {
            Ok((header, _)) => Ok((header.info.slot == slot).then_some(header.info)),
            Err(SaveError::Format(_) | SaveError::NotASave | SaveError::Json(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn list(&self) -> Result<Vec<SaveInfo>> {
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let name = entry?.file_name();
            if let Some(slot) = SaveSlot::parse(&name.to_string_lossy())
                && let Some(info) = self.info(slot)?
            {
                result.push(info);
            }
        }
        result.sort_by_key(|i| i.slot);
        Ok(result)
    }
    pub fn save<C: ContentSource>(
        &self,
        slot: SaveSlot,
        session: &GameSession<C>,
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
        let state = session.state();
        let header = Header {
            yarra_save: SAVE_FORMAT,
            info: SaveInfo {
                slot,
                title,
                playthrough: state.playthrough,
                generation: state.generation,
                game_time: state.time,
                saved_at_ms: u64::try_from(saved_at_ms)
                    .map_err(|_| game_types::Invalid("wall clock overflow".into()))?,
                sequence: self.next_sequence(slot)?,
            },
            content: session.identity().clone(),
        };
        // Write beside the slot, then rename: readers see the old save or the new one.
        let stage = self.root.join(format!(".saving-{}", OwnerId::new()));
        let written = (|| -> Result<()> {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&stage)?;
            let mut out = std::io::BufWriter::new(&mut file);
            serde_json::to_writer(&mut out, &header)?;
            out.write_all(b"\n")?;
            serde_json::to_writer(&mut out, state)?;
            out.flush()?;
            drop(out);
            file.sync_all()?;
            fs::rename(&stage, self.path(slot))?;
            fs::File::open(&self.root)?.sync_all()?;
            Ok(())
        })();
        if written.is_err() {
            let _ = fs::remove_file(&stage);
        }
        written?;
        Ok(header.info)
    }
    pub fn quicksave<C: ContentSource>(&self, session: &GameSession<C>) -> Result<SaveInfo> {
        self.save(SaveSlot::Quick, session, "Quicksave")
    }
    /// Rotate only autosaves. Manual and quick slots never participate in retention.
    pub fn autosave<C: ContentSource>(&self, session: &GameSession<C>) -> Result<SaveInfo> {
        let mut oldest = None;
        for index in 0..self.autosave_slots {
            let slot = SaveSlot::Auto(index);
            let Some(info) = self.info(slot)? else {
                return self.save(slot, session, "Autosave");
            };
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
    /// Identify the published content needed to restore this slot.
    pub fn content_identity(&self, slot: SaveSlot) -> Result<ContentIdentity> {
        Ok(read_header(&self.path(slot))?.0.content)
    }
    /// The restored state is checked in full against the supplied content.
    pub fn load<C: ContentSource>(&self, slot: SaveSlot, content: C) -> Result<GameSession<C>> {
        let (header, reader) = read_header(&self.path(slot))?;
        require(header.info.slot == slot, "slot metadata mismatch")?;
        if header.content != content.identity() {
            return Err(SaveError::ContentMismatch);
        }
        let state: SessionState = serde_json::from_reader(reader)?;
        require(
            state.playthrough == header.info.playthrough
                && state.generation == header.info.generation
                && state.time == header.info.game_time,
            "save summary differs from saved state",
        )?;
        Ok(GameSession::new(content, state)?)
    }
    fn next_sequence(&self, slot: SaveSlot) -> Result<u64> {
        let slots: Vec<_> = if matches!(slot, SaveSlot::Auto(_)) {
            (0..self.autosave_slots).map(SaveSlot::Auto).collect()
        } else {
            vec![slot]
        };
        let mut maximum = 0;
        for slot in slots {
            if let Some(info) = self.info(slot)? {
                maximum = maximum.max(info.sequence);
            }
        }
        maximum
            .checked_add(1)
            .ok_or_else(|| game_types::Invalid("save sequence overflow".into()).into())
    }
}
