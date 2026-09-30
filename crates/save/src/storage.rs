//! Coherent SQLite backup into private staging files, then atomic checkpoint publication.
use crate::{Result, SaveError, SaveInfo, SaveSlot, WorkingStore, working};
use game_types::{OwnerId, require};
use gameplay::{ContentIdentity, ContentSource, GameSession, StateStore};
use rusqlite::{
    Connection, OpenFlags,
    backup::{Backup, StepResult},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    working::configure(&c)?;
    working::check(&c)?;
    Ok(c)
}
pub(crate) fn info(path: &Path) -> Result<SaveInfo> {
    let c = open(path)?;
    let text:String=c.query_row("SELECT CASE WHEN length(CAST(info AS BLOB))<=4096 THEN info END FROM session_meta WHERE singleton=1",[],|r|r.get(0))?;
    Ok(serde_json::from_str(&text)?)
}
pub(crate) fn identity(path: &Path) -> Result<ContentIdentity> {
    let c = open(path)?;
    let data:Vec<u8>=c.query_row("SELECT CASE WHEN length(content)<=4096 THEN content END FROM session_meta WHERE singleton=1",[],|r|r.get(0))?;
    Ok(serde_json::from_slice(&data)?)
}
pub(crate) fn copy_snapshot(source: &Connection, destination: &mut Connection) -> Result<()> {
    let backup = Backup::new(source, destination)?;
    loop {
        match backup.step(64)? {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            _ => {
                return Err(
                    game_types::Invalid("checkpoint source busy; retry checkpoint".into()).into(),
                );
            }
        }
    }
}
struct Staging(PathBuf);
impl Staging {
    fn new(path: &Path) -> Result<Self> {
        let parent = path.parent().unwrap_or(Path::new("."));
        let path = parent.join(format!(".checkpoint-{}.sqlite", OwnerId::new()));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub(crate) fn write<C: ContentSource>(
    path: &Path,
    session: &GameSession<WorkingStore, C>,
    info: &SaveInfo,
) -> Result<()> {
    if path.try_exists()? {
        let previous = self::info(path)?;
        require(previous.slot == info.slot, "slot metadata mismatch")?;
    }
    let stage = Staging::new(path)?;
    let mut destination = Connection::open(&stage.0)?;
    working::configure(&destination)?;
    // Pin a read snapshot before inspecting metadata. Backup includes every nonresident row.
    let snapshot = session.store().connection.unchecked_transaction()?;
    let current = session.header()?;
    require(
        current.generation == info.generation
            && current.playthrough == info.playthrough
            && current.time == info.game_time,
        "session changed before checkpoint",
    )?;
    copy_snapshot(&snapshot, &mut destination)?;
    snapshot.rollback()?;
    // Published/restored files must not depend on staging WAL sidecars.
    destination.pragma_update(None, "journal_mode", "DELETE")?;
    destination.execute(
        "UPDATE session_meta SET info=?1 WHERE singleton=1",
        [serde_json::to_string(info)?],
    )?;
    destination.close().map_err(|(_, e)| e)?;
    fs::File::open(&stage.0)?.sync_all()?;
    fs::rename(&stage.0, path)?;
    fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}
/// Always restore into a fresh private database. A named slot is never a live session.
pub(crate) fn load<C: ContentSource>(
    path: &Path,
    slot: SaveSlot,
    content: C,
    working_path: &Path,
) -> Result<GameSession<WorkingStore, C>> {
    let source = open(path)?;
    let snapshot = source.unchecked_transaction()?;
    let (data,text):(Vec<u8>,String)=snapshot.query_row("SELECT CASE WHEN length(content)<=4096 THEN content END,CASE WHEN length(CAST(info AS BLOB))<=4096 THEN info END FROM session_meta WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
    let identity: ContentIdentity = serde_json::from_slice(&data)?;
    if identity != content.identity() {
        return Err(SaveError::ContentMismatch);
    }
    let info: SaveInfo = serde_json::from_str(&text)?;
    require(info.slot == slot, "slot metadata mismatch")?;
    let stage = Staging::new(working_path)?;
    let mut destination = Connection::open(&stage.0)?;
    working::configure(&destination)?;
    copy_snapshot(&snapshot, &mut destination)?;
    snapshot.rollback()?;
    // Published/restored files must not depend on staging WAL sidecars.
    destination.pragma_update(None, "journal_mode", "DELETE")?;
    destination.execute("UPDATE session_meta SET info=NULL", [])?;
    let store = WorkingStore::from_connection(destination)?;
    let header = store.header()?;
    require(
        header.playthrough == info.playthrough
            && header.generation == info.generation
            && header.time == info.game_time,
        "save summary differs from checkpoint",
    )?;
    drop(store);
    // No-clobber publication preserves any existing working session.
    fs::hard_link(&stage.0, working_path)?;
    let store = WorkingStore::open(working_path)?;
    Ok(GameSession::new(store, content)?)
}
