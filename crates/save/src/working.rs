//! Private working database. Records are read per operation; accepted commands never advance slots.
use crate::{Result, SaveError};
use game_types::*;
use gameplay::*;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

pub const APPLICATION_ID: i64 = 0x59525347;
pub const SCHEMA_VERSION: i64 = 7;
const SCHEMA: &str = "
CREATE TABLE session_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1), content BLOB NOT NULL CHECK(length(content)<=4096), header BLOB NOT NULL CHECK(length(header)<=4096), info TEXT CHECK(length(CAST(info AS BLOB))<=4096)) STRICT;
CREATE TABLE owners(kind TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(kind,id)) STRICT, WITHOUT ROWID;
CREATE TABLE records(kind INTEGER NOT NULL CHECK(kind BETWEEN 1 AND 12),id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),owner_kind TEXT,owner_id TEXT,role TEXT,next_expiry INTEGER,payload BLOB NOT NULL CHECK(length(payload)<=1048576),hash BLOB NOT NULL CHECK(length(hash)=32),PRIMARY KEY(kind,id),FOREIGN KEY(owner_kind,owner_id) REFERENCES owners(kind,id)) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX inventory_roles ON records(owner_kind,owner_id,role) WHERE kind=2;
CREATE INDEX expiration_queue ON records(next_expiry,id) WHERE kind=1 AND next_expiry IS NOT NULL;
CREATE TABLE item_locations(item TEXT PRIMARY KEY, inventory_kind INTEGER NOT NULL DEFAULT 2 CHECK(inventory_kind=2),inventory TEXT NOT NULL, FOREIGN KEY(inventory_kind,inventory) REFERENCES records(kind,id)) STRICT, WITHOUT ROWID;
CREATE INDEX items_by_inventory ON item_locations(inventory);
CREATE TABLE movements(trigger TEXT PRIMARY KEY, actor TEXT NOT NULL UNIQUE, deadline INTEGER NOT NULL CHECK(deadline>=0), kind INTEGER NOT NULL DEFAULT 12 CHECK(kind=12), FOREIGN KEY(kind,trigger) REFERENCES records(kind,id)) STRICT, WITHOUT ROWID;
CREATE INDEX movement_deadlines ON movements(deadline,trigger);
CREATE TABLE pending_events(generation INTEGER NOT NULL CHECK(generation>0), ordinal INTEGER NOT NULL CHECK(ordinal BETWEEN 0 AND 255), payload BLOB NOT NULL CHECK(length(payload)<=4096), hash BLOB NOT NULL CHECK(length(hash)=32), PRIMARY KEY(generation,ordinal)) STRICT, WITHOUT ROWID;
CREATE TABLE facts(id TEXT PRIMARY KEY) STRICT, WITHOUT ROWID;
";
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StateStats {
    pub decoded_records: u64,
    pub payload_bytes: u64,
    /// Successfully staged record writes, including transactions later rolled back.
    pub written_records: u64,
    pub peak_working_records: usize,
    pub peak_working_bytes: usize,
}

pub struct WorkingStore {
    pub(crate) connection: Connection,
    identity: ContentIdentity,
    stats: Cell<StateStats>,
}
pub type StoredSession<C> = GameSession<WorkingStore, C>;
fn runtime(error: impl std::fmt::Display) -> GameplayError {
    GameplayError::Runtime(error.to_string())
}
fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(v)?)
}
fn decode<T: DeserializeOwned>(v: &[u8]) -> Result<T> {
    Ok(serde_json::from_slice(v)?)
}
pub(crate) fn configure(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA trusted_schema=OFF; PRAGMA mmap_size=0; PRAGMA cache_size=-512; PRAGMA busy_timeout=1000; PRAGMA synchronous=FULL;")?;
    Ok(())
}
pub(crate) fn check(connection: &Connection) -> Result<()> {
    let id: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    if id != APPLICATION_ID {
        return Err(SaveError::WrongDatabase);
    }
    let version = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != SCHEMA_VERSION {
        return Err(SaveError::Schema(version));
    }
    Ok(())
}
impl WorkingStore {
    /// Eager new-game import only. Runtime opening/commands/checkpoints never use this path.
    pub fn in_memory(content: &GameContent, state: &SessionState) -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?, content, state)
    }
    pub fn create(
        path: impl AsRef<Path>,
        content: &GameContent,
        state: &SessionState,
    ) -> Result<Self> {
        let path = path.as_ref();
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        let result = Connection::open(path)
            .map_err(SaveError::from)
            .and_then(|c| Self::initialize(c, content, state));
        if result.is_err() {
            let _ = std::fs::remove_file(path);
        }
        result
    }
    fn initialize(
        mut connection: Connection,
        content: &GameContent,
        state: &SessionState,
    ) -> Result<Self> {
        state.validate(content)?;
        let identity = ContentIdentity {
            manifest: content.manifest.clone(),
            fingerprint: content.fingerprint()?,
        };
        let header = SessionHeader::capture(state);
        header.validate()?;
        configure(&connection)?;
        let tx = connection.transaction()?;
        tx.execute_batch(SCHEMA)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.execute(
            "INSERT INTO session_meta VALUES(1,?1,?2,NULL)",
            params![encode(&identity)?, encode(&header)?],
        )?;
        for owner in state
            .owners
            .iter()
            .cloned()
            .chain(state.actors.iter().map(|a| OwnerRef::actor(a.id)))
        {
            tx.execute(
                "INSERT INTO owners VALUES(?1,?2)",
                params![owner.kind, owner.id.to_string()],
            )?;
        }
        for o in &state.world.objects {
            put(&tx, Record::Object(o), None)?;
        }
        for l in &state.world.locations {
            put(&tx, Record::Location(l), None)?;
        }
        for t in &state.world.triggers {
            put(&tx, Record::Trigger(t), None)?;
        }
        for e in &state.world.pending {
            put_event(&tx, e)?;
        }
        for a in &state.actors {
            put(&tx, Record::Actor(a), None)?;
        }
        for i in &state.inventories {
            put(&tx, Record::Inventory(i), None)?;
            index_items(&tx, i)?;
        }
        for w in &state.wallets {
            put(&tx, Record::Wallet(w), None)?;
        }
        for c in &state.conversations {
            put(&tx, Record::Conversation(c), None)?;
        }
        for h in &state.histories {
            put(&tx, Record::History(h), None)?;
        }
        for c in &state.claims {
            put(&tx, Record::Claim(c), None)?;
        }
        for q in &state.quests {
            put(&tx, Record::Quest(q), None)?;
        }
        for r in &state.relationships {
            put(&tx, Record::Relationship(r), None)?;
        }
        for i in &state.interactions {
            put(&tx, Record::Interaction(i), None)?;
        }
        for f in &state.facts {
            tx.execute("INSERT INTO facts VALUES(?1)", [f.as_str()])?;
        }
        tx.commit()?;
        Ok(Self {
            connection,
            identity,
            stats: Cell::default(),
        })
    }
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let connection =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        Self::from_connection(connection)
    }
    pub(crate) fn from_connection(connection: Connection) -> Result<Self> {
        configure(&connection)?;
        check(&connection)?;
        require(
            connection.query_row(
                "SELECT info IS NULL FROM session_meta WHERE singleton=1",
                [],
                |r| r.get::<_, bool>(0),
            )?,
            "a checkpoint must be restored into a private working database",
        )?;
        let identity = decode(&connection.query_row("SELECT CASE WHEN length(content)<=4096 THEN content END FROM session_meta WHERE singleton=1",[],|r|r.get::<_,Vec<u8>>(0))?)?;
        let store = Self {
            connection,
            identity,
            stats: Cell::default(),
        };
        store.header()?.validate()?;
        Ok(store)
    }
    pub fn stats(&self) -> StateStats {
        self.stats.get()
    }
    /// Explicit tooling export; never called by commands, save or load. Bounded by a caller budget.
    pub fn export_for_tools(&self, max_records: usize) -> Result<SessionState> {
        require(
            max_records <= 100_000,
            "tool export record budget too large",
        )?;
        let tx = self.connection.unchecked_transaction()?;
        let mut state = read_header(&tx)?.empty_state();
        let mut stmt = tx.prepare("SELECT kind,id,CASE WHEN length(payload)<=1048576 THEN payload END,hash FROM records ORDER BY kind,id LIMIT ?1")?;
        let mut rows = stmt.query([max_records as i64 + 1])?;
        let mut count = 0;
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            count += 1;
            require(count <= max_records, "tool export record budget exceeded")?;
            let data: Vec<u8> = row.get(2)?;
            bytes += data.len();
            require(
                bytes <= 64 * 1024 * 1024,
                "tool export byte budget exceeded",
            )?;
            require(
                blake3::hash(&data).as_bytes().as_slice() == row.get::<_, Vec<u8>>(3)?,
                "record checksum mismatch",
            )?;
            match row.get::<_, i64>(0)? {
                10 => state.world.objects.push(decode(&data)?),
                11 => state.world.locations.push(decode(&data)?),
                12 => state.world.triggers.push(decode(&data)?),
                1 => state.actors.push(decode(&data)?),
                2 => state.inventories.push(decode(&data)?),
                3 => state.wallets.push(decode(&data)?),
                4 => state.conversations.push(decode(&data)?),
                8 => state.histories.push(decode(&data)?),
                9 => state.claims.push(decode(&data)?),
                5 => state.quests.push(decode(&data)?),
                6 => state.relationships.push(decode(&data)?),
                7 => state.interactions.push(decode(&data)?),
                _ => return Err(Invalid("invalid record kind".into()).into()),
            }
        }
        drop(rows);
        drop(stmt);
        let mut stmt = tx.prepare("SELECT generation,ordinal,CASE WHEN length(payload)<=4096 THEN payload END,hash FROM pending_events ORDER BY generation,ordinal LIMIT 4097")?;
        for e in stmt.query_map([], event_row)? {
            let event = e??;
            bytes += encode(&event)?.len();
            require(
                bytes <= 64 * 1024 * 1024 && count + state.world.pending.len() < max_records,
                "tool export event budget exceeded",
            )?;
            state.world.pending.push(event);
        }
        require(
            state.world.pending.len() <= MAX_PENDING_EVENTS,
            "pending event budget exceeded",
        )?;
        drop(stmt);
        let mut stmt = tx.prepare(
            "SELECT kind,id FROM owners WHERE kind<>'actor' ORDER BY kind,id LIMIT 10001",
        )?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (kind, id) = row?;
            state
                .owners
                .push(OwnerRef::new(kind, OwnerId::try_from(id)?)?);
        }
        let mut stmt = tx.prepare("SELECT id FROM facts ORDER BY id LIMIT 10001")?;
        for row in stmt.query_map([], |r| r.get::<_, String>(0))? {
            state.facts.insert(Key::new(row?)?);
        }
        require(
            state.owners.len() <= 10000 && state.facts.len() <= 10000,
            "tool export scope exceeded",
        )?;
        Ok(state)
    }
}
fn read_header(c: &Connection) -> Result<SessionHeader> {
    decode(&c.query_row(
        "SELECT CASE WHEN length(header)<=4096 THEN header END FROM session_meta WHERE singleton=1",
        [],
        |r| r.get::<_, Vec<u8>>(0),
    )?)
}
impl StateStore for WorkingStore {
    type Transaction<'a> = WorkingTransaction<'a>;
    fn identity(&self) -> &ContentIdentity {
        &self.identity
    }
    fn header(&self) -> gameplay::Result<SessionHeader> {
        read_header(&self.connection).map_err(runtime)
    }
    fn begin(&mut self) -> gameplay::Result<Self::Transaction<'_>> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(runtime)?;
        let header = read_header(&tx).map_err(runtime)?;
        Ok(WorkingTransaction {
            tx,
            header,
            stats: &self.stats,
            failed: false,
            enqueued: 0,
        })
    }
}
pub struct WorkingTransaction<'a> {
    tx: Transaction<'a>,
    header: SessionHeader,
    stats: &'a Cell<StateStats>,
    failed: bool,
    enqueued: u32,
}
fn conversation_id(k: ConversationKey) -> String {
    format!("{}/{}/{}", k.dialogue, k.participant, k.speaker)
}
fn record_id(k: &RecordKey) -> (i64, String) {
    match k {
        RecordKey::Object(id) => (10, id.to_string()),
        RecordKey::Location(id) => (11, id.to_string()),
        RecordKey::Trigger(id) => (12, id.to_string()),
        RecordKey::History(k) => (8, serde_json::to_string(k).expect("history key")),
        RecordKey::Claim(k) => (9, serde_json::to_string(k).expect("claim key")),
        RecordKey::Quest(id) => (5, id.to_string()),
        RecordKey::Relationship(k) => (6, format!("{}/{}", k.from, k.to)),
        RecordKey::Interaction(k) => (7, format!("{}/{}", k.participant, k.speaker)),
        RecordKey::Actor(id) => (1, id.to_string()),
        RecordKey::Inventory(id) => (2, id.to_string()),
        RecordKey::Wallet(id) => (3, id.to_string()),
        RecordKey::Conversation(id) => (4, conversation_id(*id)),
    }
}
impl WorkingTransaction<'_> {
    fn read<T: DeserializeOwned>(
        &self,
        key: &RecordKey,
        bytes: &mut usize,
        versions: &mut BTreeMap<RecordKey, u64>,
    ) -> Result<Option<T>> {
        require(
            versions.len() < MAX_WORKING_RECORDS,
            "working record budget exceeded",
        )?;
        let (kind, id) = record_id(key);
        let row = self.tx.query_row("SELECT revision,CASE WHEN length(payload)<=1048576 THEN payload END,hash FROM records WHERE kind=?1 AND id=?2", params![kind,id], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,Vec<u8>>(2)?))).optional()?;
        let Some((revision, data, hash)) = row else {
            versions.insert(key.clone(), 0);
            let mut stats = self.stats.get();
            stats.peak_working_records = stats.peak_working_records.max(versions.len());
            self.stats.set(stats);
            return Ok(None);
        };
        *bytes += data.len();
        require(*bytes <= MAX_WORKING_BYTES, "working byte budget exceeded")?;
        require(
            blake3::hash(&data).as_bytes().as_slice() == hash,
            "record checksum mismatch",
        )?;
        require(revision > 0, "invalid record revision")?;
        versions.insert(key.clone(), revision as u64);
        let mut stats = self.stats.get();
        stats.decoded_records += 1;
        stats.payload_bytes += data.len() as u64;
        stats.peak_working_records = stats.peak_working_records.max(versions.len());
        stats.peak_working_bytes = stats.peak_working_bytes.max(*bytes);
        self.stats.set(stats);
        Ok(Some(decode(&data)?))
    }
    fn load_inner(&self, request: &StateRequest) -> Result<WorkingSet> {
        require(
            request.objects.len()
                + request.locations.len()
                + request.triggers.len()
                + request.histories.len()
                + request.claims.len()
                + request.quests.len()
                + request.relationships.len()
                + request.interactions.len()
                + request.actors.len()
                + request.inventories.len()
                + request.wallets.len()
                + request.conversations.len()
                + request.facts.len()
                <= MAX_WORKING_RECORDS,
            "request budget exceeded",
        )?;
        let mut state = self.header.empty_state();
        let mut versions = BTreeMap::new();
        let mut bytes = 0;
        let mut actors = request.actors.clone();
        let mut inventories = request.inventories.clone();
        let mut owners = BTreeSet::new();
        let mut conversations = request.conversations.clone();
        for id in &request.objects {
            let o = self
                .read::<ObjectState>(&RecordKey::Object(*id), &mut bytes, &mut versions)?
                .unwrap_or(ObjectState {
                    id: *id,
                    locked: false,
                    open: false,
                    destroyed: false,
                });
            require(o.id == *id, "object identity mismatch")?;
            state.world.objects.push(o);
        }
        for id in &request.locations {
            let l = self
                .read::<LocationState>(&RecordKey::Location(*id), &mut bytes, &mut versions)?
                .unwrap_or_else(|| LocationState::initial(*id));
            require(l.actor == *id, "location identity mismatch")?;
            actors.insert(*id);
            state.world.locations.push(l);
        }
        for id in &request.triggers {
            let t = self
                .read::<TriggerState>(&RecordKey::Trigger(*id), &mut bytes, &mut versions)?
                .unwrap_or_else(|| TriggerState::initial(*id));
            require(t.id == *id, "trigger identity mismatch")?;
            let indexed: Option<(String, i64)> = self
                .tx
                .query_row(
                    "SELECT actor,deadline FROM movements WHERE trigger=?1",
                    [id.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            require(
                indexed
                    == t.movement
                        .as_ref()
                        .map(|m| (m.actor.to_string(), m.deadline.0 as i64)),
                "movement index mismatch",
            )?;
            state.world.triggers.push(t);
        }
        for key in &request.histories {
            let h = self
                .read::<dialogue::History>(&RecordKey::History(*key), &mut bytes, &mut versions)?
                .unwrap_or_else(|| dialogue::History::empty(*key));
            require(h.key == *key, "history identity mismatch")?;
            actors.extend(key.scope.actors());
            state.histories.push(h);
        }
        for key in &request.claims {
            let c = self
                .read::<dialogue::Claim>(&RecordKey::Claim(*key), &mut bytes, &mut versions)?
                .unwrap_or(dialogue::Claim {
                    key: *key,
                    claimed: false,
                });
            require(c.key == *key, "claim identity mismatch")?;
            actors.extend(key.scope.actors());
            state.claims.push(c);
        }
        for id in &request.quests {
            let q = self
                .read::<quests::Progress>(&RecordKey::Quest(*id), &mut bytes, &mut versions)?
                .unwrap_or_else(|| quests::Progress::new(*id));
            require(q.quest == *id, "quest identity mismatch")?;
            state.quests.push(q);
        }
        for key in &request.relationships {
            actors.extend([key.from, key.to]);
            let r = self
                .read::<actors::Relationship>(
                    &RecordKey::Relationship(*key),
                    &mut bytes,
                    &mut versions,
                )?
                .unwrap_or_else(|| actors::Relationship::neutral(*key));
            require(r.key == *key, "relationship identity mismatch")?;
            state.relationships.push(r);
        }
        for key in &request.interactions {
            actors.extend([key.participant, key.speaker]);
            let i = self
                .read::<dialogue::Interaction>(
                    &RecordKey::Interaction(*key),
                    &mut bytes,
                    &mut versions,
                )?
                .unwrap_or(dialogue::Interaction {
                    key: *key,
                    current: None,
                });
            require(i.key == *key, "interaction identity mismatch")?;
            if let Some(selection) = &i.current {
                conversations.insert(ConversationKey {
                    dialogue: selection.dialogue,
                    participant: key.participant,
                    speaker: key.speaker,
                });
            }
            state.interactions.push(i);
        }
        for id in &conversations {
            actors.extend([id.participant, id.speaker]);
            if let Some(c) = self.read::<dialogue::Conversation>(
                &RecordKey::Conversation(*id),
                &mut bytes,
                &mut versions,
            )? {
                require(
                    ConversationKey::of(&c) == *id,
                    "conversation identity mismatch",
                )?;
                actors.extend(c.bindings.values());
                state.conversations.push(c);
            }
        }
        for id in &request.wallets {
            let w = self
                .read::<inventory::Wallet>(&RecordKey::Wallet(*id), &mut bytes, &mut versions)?
                .ok_or_else(|| Invalid("unknown wallet".into()))?;
            require(w.id == *id, "wallet identity mismatch")?;
            let indexed: (String, String) = self.tx.query_row(
                "SELECT CASE WHEN length(CAST(owner_kind AS BLOB))<=96 THEN owner_kind END,CASE WHEN length(owner_id)=36 THEN owner_id END FROM records WHERE kind=3 AND id=?1",
                [id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            require(
                indexed == (w.owner.kind.clone(), w.owner.id.to_string()),
                "wallet owner index mismatch",
            )?;
            owners.insert((w.owner.kind.clone(), w.owner.id));
            state.wallets.push(w);
        }
        // An inventory owner adds an actor, and an actor adds its carried inventory. Resolve to closure.
        loop {
            if let Some(id) = inventories
                .iter()
                .find(|id| !state.inventories.iter().any(|i| i.id == **id))
                .copied()
            {
                let i = self
                    .read::<inventory::Inventory>(
                        &RecordKey::Inventory(id),
                        &mut bytes,
                        &mut versions,
                    )?
                    .ok_or_else(|| Invalid("unknown inventory".into()))?;
                require(i.id == id, "inventory identity mismatch")?;
                let indexed: (String, String, String) = self.tx.query_row(
                    "SELECT CASE WHEN length(CAST(owner_kind AS BLOB))<=96 THEN owner_kind END,CASE WHEN length(owner_id)=36 THEN owner_id END,CASE WHEN length(CAST(role AS BLOB))<=96 THEN role END FROM records WHERE kind=2 AND id=?1",
                    [id.to_string()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                require(
                    indexed == (i.owner.kind.clone(), i.owner.id.to_string(), i.role.clone()),
                    "inventory owner index mismatch",
                )?;
                let mut statement = self.tx.prepare(
                    "SELECT CASE WHEN length(item)=36 THEN item END FROM item_locations WHERE inventory=?1 ORDER BY item LIMIT 4097",
                )?;
                let indexed = statement
                    .query_map([id.to_string()], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<BTreeSet<_>, _>>()?;
                let expected: BTreeSet<_> = i.entries.iter().map(|e| e.id.to_string()).collect();
                require(indexed == expected, "inventory entry index mismatch")?;
                owners.insert((i.owner.kind.clone(), i.owner.id));
                state.inventories.push(i);
                continue;
            }
            for (kind, id) in &owners {
                if kind == "actor" {
                    actors.insert(ActorId(id.0));
                }
            }
            if let Some(id) = actors
                .iter()
                .find(|id| !state.actors.iter().any(|a| a.id == **id))
                .copied()
            {
                let a = self
                    .read::<actors::Actor>(&RecordKey::Actor(id), &mut bytes, &mut versions)?
                    .ok_or_else(|| Invalid("unknown actor".into()))?;
                require(a.id == id, "actor identity mismatch")?;
                let expiry: Option<i64> = self.tx.query_row(
                    "SELECT next_expiry FROM records WHERE kind=1 AND id=?1",
                    [id.to_string()],
                    |r| r.get(0),
                )?;
                let expected = a
                    .effects
                    .iter()
                    .map(|e| e.expires_at.0)
                    .min()
                    .map(i64::try_from)
                    .transpose()
                    .map_err(|_| Invalid("invalid effect expiry".into()))?;
                require(expiry == expected, "actor expiration index mismatch")?;
                let bag:String=self.tx.query_row("SELECT CASE WHEN length(id)=36 THEN id END FROM records INDEXED BY inventory_roles WHERE kind=2 AND owner_kind='actor' AND owner_id=?1 AND role='carried'",[id.to_string()],|r|r.get(0))?;
                inventories.insert(InventoryId::try_from(bag)?);
                state.actors.push(a);
                continue;
            }
            break;
        }
        for (kind, id) in owners {
            require(
                self.tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM owners WHERE kind=?1 AND id=?2)",
                    params![kind, id.to_string()],
                    |r| r.get::<_, bool>(0),
                )?,
                "unknown owner",
            )?;
            if kind != "actor" {
                state.owners.push(OwnerRef::new(kind, id)?);
            }
        }
        Ok(WorkingSet { state, versions })
    }
    fn stage_inner(&self, before: &WorkingSet, after: &SessionState) -> Result<()> {
        let old = &before.state;
        require(
            after.world.objects.len() == old.world.objects.len()
                && after.world.locations.len() == old.world.locations.len()
                && after.world.triggers.len() == old.world.triggers.len()
                && after.histories.len() == old.histories.len()
                && after.claims.len() == old.claims.len()
                && after.actors.len() == old.actors.len()
                && after.inventories.len() == old.inventories.len()
                && after.wallets.len() == old.wallets.len()
                && after.quests.len() == old.quests.len()
                && after.relationships.len() == old.relationships.len()
                && after.interactions.len() == old.interactions.len(),
            "command changed record scope",
        )?;
        // Remove old indexes for all changed bags first, allowing atomic moves between bags.
        for i in &after.inventories {
            if old.inventory(i.id)? != i {
                self.tx.execute(
                    "DELETE FROM item_locations WHERE inventory=?1",
                    [i.id.to_string()],
                )?;
            }
        }
        let mut written = 0;
        for o in &after.world.objects {
            if old.world.object(o.id)? != o {
                put(
                    &self.tx,
                    Record::Object(o),
                    Some(before.versions[&RecordKey::Object(o.id)]),
                )?;
                written += 1;
            }
        }
        for l in &after.world.locations {
            if old.world.location(l.actor)? != l {
                put(
                    &self.tx,
                    Record::Location(l),
                    Some(before.versions[&RecordKey::Location(l.actor)]),
                )?;
                written += 1;
            }
        }
        for t in &after.world.triggers {
            if old.world.trigger(t.id)? != t {
                put(
                    &self.tx,
                    Record::Trigger(t),
                    Some(before.versions[&RecordKey::Trigger(t.id)]),
                )?;
                written += 1;
            }
        }
        for a in &after.actors {
            if old.actor(a.id)? != a {
                put(
                    &self.tx,
                    Record::Actor(a),
                    Some(before.versions[&RecordKey::Actor(a.id)]),
                )?;
                written += 1;
            }
        }
        for i in &after.inventories {
            if old.inventory(i.id)? != i {
                put(
                    &self.tx,
                    Record::Inventory(i),
                    Some(before.versions[&RecordKey::Inventory(i.id)]),
                )?;
                index_items(&self.tx, i)?;
                written += 1;
            }
        }
        for w in &after.wallets {
            if old.wallet(w.id)? != w {
                put(
                    &self.tx,
                    Record::Wallet(w),
                    Some(before.versions[&RecordKey::Wallet(w.id)]),
                )?;
                written += 1;
            }
        }
        for c in &after.conversations {
            let key = ConversationKey::of(c);
            if old
                .conversations
                .iter()
                .find(|v| ConversationKey::of(v) == key)
                != Some(c)
            {
                put(
                    &self.tx,
                    Record::Conversation(c),
                    before.versions.get(&RecordKey::Conversation(key)).copied(),
                )?;
                written += 1;
            }
        }
        for h in &after.histories {
            if old.history(h.key)? != h {
                put(
                    &self.tx,
                    Record::History(h),
                    before.versions.get(&RecordKey::History(h.key)).copied(),
                )?;
                written += 1;
            }
        }
        for c in &after.claims {
            if old.claim(c.key)? != c {
                put(
                    &self.tx,
                    Record::Claim(c),
                    before.versions.get(&RecordKey::Claim(c.key)).copied(),
                )?;
                written += 1;
            }
        }
        for q in &after.quests {
            if old.quest(q.quest)? != q {
                put(
                    &self.tx,
                    Record::Quest(q),
                    before.versions.get(&RecordKey::Quest(q.quest)).copied(),
                )?;
                written += 1;
            }
        }
        for r in &after.relationships {
            if old.relationship(r.key)? != r {
                put(
                    &self.tx,
                    Record::Relationship(r),
                    before
                        .versions
                        .get(&RecordKey::Relationship(r.key))
                        .copied(),
                )?;
                written += 1;
            }
        }
        for i in &after.interactions {
            if old.interaction(i.key)? != i {
                put(
                    &self.tx,
                    Record::Interaction(i),
                    before.versions.get(&RecordKey::Interaction(i.key)).copied(),
                )?;
                written += 1;
            }
        }
        for f in old.facts.difference(&after.facts) {
            self.tx
                .execute("DELETE FROM facts WHERE id=?1", [f.as_str()])?;
        }
        for f in after.facts.difference(&old.facts) {
            self.tx
                .execute("INSERT INTO facts VALUES(?1)", [f.as_str()])?;
        }
        let mut stats = self.stats.get();
        stats.written_records += written;
        self.stats.set(stats);
        Ok(())
    }
}
impl StateTransaction for WorkingTransaction<'_> {
    fn header(&self) -> &SessionHeader {
        &self.header
    }
    fn load(&mut self, request: &StateRequest) -> gameplay::Result<WorkingSet> {
        let result = self.load_inner(request).map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn facts(&mut self, keys: &BTreeSet<Key>) -> gameplay::Result<BTreeSet<Key>> {
        require(keys.len() <= 4096, "fact query budget exceeded")?;
        let mut found = BTreeSet::new();
        for key in keys {
            if self
                .tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM facts WHERE id=?1)",
                    [key.as_str()],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(runtime)?
            {
                found.insert(key.clone());
            }
        }
        Ok(found)
    }
    fn enqueue(&mut self, signals: &BTreeSet<WorldSignal>) -> gameplay::Result<()> {
        let result = (|| -> Result<()> {
            require(
                self.enqueued as usize + signals.len() <= 256,
                "command event budget exceeded",
            )?;
            let count: u32 = self.tx.query_row(
                "SELECT COUNT(*) FROM (SELECT 1 FROM pending_events LIMIT 4097)",
                [],
                |r| r.get(0),
            )?;
            require(
                count as usize + signals.len() <= MAX_PENDING_EVENTS,
                "pending event queue full; process work before accepting more",
            )?;
            for signal in signals {
                let event = PendingEvent {
                    id: EventId {
                        generation: self.header.generation,
                        ordinal: self.enqueued,
                    },
                    signal: signal.clone(),
                    after: None,
                };
                put_event(&self.tx, &event)?;
                self.enqueued += 1;
            }
            Ok(())
        })()
        .map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn next_event(&mut self) -> gameplay::Result<Option<PendingEvent>> {
        let result = (|| -> Result<Option<PendingEvent>> {
            self.tx.query_row("SELECT generation,ordinal,CASE WHEN length(payload)<=4096 THEN payload END,hash FROM pending_events ORDER BY generation,ordinal LIMIT 1", [], event_row).optional()?.transpose()
        })().map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn advance_event(
        &mut self,
        event: &PendingEvent,
        after: Option<TriggerId>,
    ) -> gameplay::Result<()> {
        let result = (|| -> Result<()> {
            require(
                after.is_none_or(|id| event.after.is_none_or(|previous| id > previous)),
                "event cursor did not advance",
            )?;
            let removed = self.tx.execute(
                "DELETE FROM pending_events WHERE generation=?1 AND ordinal=?2 AND payload=?3",
                params![event.id.generation as i64, event.id.ordinal, encode(event)?],
            )?;
            require(removed == 1, "stale pending event")?;
            if let Some(id) = after {
                let mut next = event.clone();
                next.after = Some(id);
                put_event(&self.tx, &next)?;
            }
            Ok(())
        })()
        .map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn movement_owner(&mut self, actor: ActorId) -> gameplay::Result<Option<TriggerId>> {
        let result = (|| -> Result<Option<TriggerId>> {
            let id: Option<String> = self
                .tx
                .query_row(
                    "SELECT trigger FROM movements WHERE actor=?1",
                    [actor.to_string()],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(id.map(TriggerId::try_from).transpose()?)
        })()
        .map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn next_movement(
        &mut self,
        after: Option<TriggerId>,
        through: Option<GameTime>,
    ) -> gameplay::Result<Option<TriggerId>> {
        let result = (|| -> Result<Option<TriggerId>> {
            let id: Option<String> = if let Some(time) = through {
                self.tx.query_row("SELECT trigger FROM movements INDEXED BY movement_deadlines WHERE deadline<=?1 ORDER BY deadline,trigger LIMIT 1", [time.0 as i64], |r| r.get(0)).optional()?
            } else {
                self.tx.query_row("SELECT trigger FROM movements WHERE trigger>?1 ORDER BY trigger LIMIT 1", [after.map(|id| id.to_string()).unwrap_or_default()], |r| r.get(0)).optional()?
            };
            Ok(id.map(TriggerId::try_from).transpose()?)
        })().map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn next_expiring_actor(&mut self, through: GameTime) -> gameplay::Result<Option<ActorId>> {
        self.tx.query_row("SELECT CASE WHEN length(id)=36 THEN id END FROM records INDEXED BY expiration_queue WHERE kind=1 AND next_expiry IS NOT NULL AND next_expiry<=?1 ORDER BY next_expiry,id LIMIT 1",[i64::try_from(through.0).map_err(runtime)?],|r|r.get::<_,String>(0)).optional().map_err(runtime)?.map(ActorId::try_from).transpose().map_err(Into::into)
    }
    fn stage(&mut self, before: &WorkingSet, after: &SessionState) -> gameplay::Result<()> {
        let result = self.stage_inner(before, after).map_err(runtime);
        self.failed |= result.is_err();
        result
    }
    fn commit(self, header: &SessionHeader) -> gameplay::Result<()> {
        require(!self.failed, "cannot commit a failed working transaction")?;
        header.validate()?;
        require(
            header.playthrough == self.header.playthrough
                && header.generation == self.header.generation + 1
                && header.time >= self.header.time,
            "invalid session transition",
        )?;
        let changed = self
            .tx
            .execute(
                "UPDATE session_meta SET header=?1,info=NULL WHERE singleton=1 AND header=?2",
                params![
                    encode(header).map_err(runtime)?,
                    encode(&self.header).map_err(runtime)?
                ],
            )
            .map_err(runtime)?;
        require(changed == 1, "stale session generation")?;
        self.tx.commit().map_err(runtime)
    }
}
enum Record<'a> {
    Object(&'a ObjectState),
    Location(&'a LocationState),
    Trigger(&'a TriggerState),
    History(&'a dialogue::History),
    Claim(&'a dialogue::Claim),
    Quest(&'a quests::Progress),
    Relationship(&'a actors::Relationship),
    Interaction(&'a dialogue::Interaction),
    Actor(&'a actors::Actor),
    Inventory(&'a inventory::Inventory),
    Wallet(&'a inventory::Wallet),
    Conversation(&'a dialogue::Conversation),
}
fn put(tx: &Transaction<'_>, record: Record<'_>, revision: Option<u64>) -> Result<()> {
    let movement = if let Record::Trigger(t) = &record {
        Some((t.id, t.movement.clone()))
    } else {
        None
    };
    let (key, data, owner, role, expiry) = match record {
        Record::Object(o) => (RecordKey::Object(o.id), encode(o)?, None, None, None),
        Record::Location(l) => (RecordKey::Location(l.actor), encode(l)?, None, None, None),
        Record::Trigger(t) => (RecordKey::Trigger(t.id), encode(t)?, None, None, None),
        Record::History(h) => (RecordKey::History(h.key), encode(h)?, None, None, None),
        Record::Claim(c) => (RecordKey::Claim(c.key), encode(c)?, None, None, None),
        Record::Quest(q) => (RecordKey::Quest(q.quest), encode(q)?, None, None, None),
        Record::Relationship(r) => (RecordKey::Relationship(r.key), encode(r)?, None, None, None),
        Record::Interaction(i) => (RecordKey::Interaction(i.key), encode(i)?, None, None, None),
        Record::Actor(a) => (
            RecordKey::Actor(a.id),
            encode(a)?,
            None,
            None,
            a.effects.iter().map(|e| e.expires_at.0).min(),
        ),
        Record::Inventory(i) => (
            RecordKey::Inventory(i.id),
            encode(i)?,
            Some(&i.owner),
            Some(i.role.as_str()),
            None,
        ),
        Record::Wallet(w) => (
            RecordKey::Wallet(w.id),
            encode(w)?,
            Some(&w.owner),
            None,
            None,
        ),
        Record::Conversation(c) => (
            RecordKey::Conversation(ConversationKey::of(c)),
            encode(c)?,
            None,
            None,
            None,
        ),
    };
    require(
        data.len() <= MAX_RECORD_BYTES,
        "record byte budget exceeded",
    )?;
    let expiry = expiry
        .map(i64::try_from)
        .transpose()
        .map_err(|_| Invalid("effect expiry exceeds storage range".into()))?;
    let (kind, id) = record_id(&key);
    let hash = blake3::hash(&data);
    let changed = if let Some(revision) = revision.filter(|r| *r > 0) {
        tx.execute("UPDATE records SET revision=revision+1,owner_kind=?3,owner_id=?4,role=?5,next_expiry=?6,payload=?7,hash=?8 WHERE kind=?1 AND id=?2 AND revision=?9",params![kind,id,owner.map(|o|o.kind.as_str()),owner.map(|o|o.id.to_string()),role,expiry,data,hash.as_bytes(),i64::try_from(revision).map_err(|_|Invalid("record revision overflow".into()))?])?
    } else {
        tx.execute(
            "INSERT INTO records VALUES(?1,?2,1,?3,?4,?5,?6,?7,?8)",
            params![
                kind,
                id,
                owner.map(|o| o.kind.as_str()),
                owner.map(|o| o.id.to_string()),
                role,
                expiry,
                data,
                hash.as_bytes()
            ],
        )?
    };
    require(changed == 1, "stale record revision")?;
    if let Some((id, movement)) = movement {
        tx.execute("DELETE FROM movements WHERE trigger=?1", [id.to_string()])?;
        if let Some(m) = movement {
            tx.execute(
                "INSERT INTO movements(trigger,actor,deadline) VALUES(?1,?2,?3)",
                params![id.to_string(), m.actor.to_string(), m.deadline.0 as i64],
            )?;
        }
    }
    Ok(())
}
fn index_items(tx: &Transaction<'_>, inventory: &inventory::Inventory) -> Result<()> {
    for e in &inventory.entries {
        tx.execute(
            "INSERT INTO item_locations(item,inventory) VALUES(?1,?2)",
            params![e.id.to_string(), inventory.id.to_string()],
        )?;
    }
    Ok(())
}

fn put_event(tx: &Transaction<'_>, event: &PendingEvent) -> Result<()> {
    let data = encode(event)?;
    require(
        data.len() <= 4096 && event.id.generation > 0 && event.id.generation < i64::MAX as u64,
        "invalid pending event",
    )?;
    tx.execute(
        "INSERT INTO pending_events VALUES(?1,?2,?3,?4)",
        params![
            event.id.generation as i64,
            event.id.ordinal,
            data,
            blake3::hash(&data).as_bytes().as_slice()
        ],
    )?;
    Ok(())
}
fn event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<PendingEvent>> {
    let generation: i64 = row.get(0)?;
    let ordinal: u32 = row.get(1)?;
    let data: Vec<u8> = row.get(2)?;
    let hash: Vec<u8> = row.get(3)?;
    Ok((|| {
        require(
            data.len() <= 4096 && blake3::hash(&data).as_bytes().as_slice() == hash,
            "pending event checksum mismatch",
        )?;
        let event: PendingEvent = decode(&data)?;
        require(
            generation > 0
                && event.id
                    == EventId {
                        generation: generation as u64,
                        ordinal,
                    },
            "pending event identity mismatch",
        )?;
        Ok(event)
    })())
}
