//! Synchronous, read-only repository intended for a content I/O worker.
//! Opening pins one SQLite snapshot; no source loading or scenario execution occurs.
use crate::{
    BUNDLE_APPLICATION_ID, BUNDLE_SCHEMA_VERSION, ContentError, Result, asset::*,
    bundle::BundleManifest,
};
use game_types::{DialogueId, ItemDefinitionId, require};
use gameplay::{dialogue, inventory, quests, rules};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
};

#[derive(Debug, Clone)]
pub struct RepositoryLimits {
    pub max_bundle_bytes: u64,
    pub max_asset_bytes: usize,
    pub max_roots: usize,
    pub max_resolved_assets: usize,
    pub max_cached_assets: usize,
    /// Conservative accounting: 16 × encoded bytes + 4 KiB per decoded record.
    /// Includes headroom for typed allocations/temporary decoding, not a process RSS measurement.
    pub cache_charge_bytes: usize,
    /// SQLite page-cache target, separate from application cache accounting.
    pub sqlite_cache_kib: u32,
}
impl Default for RepositoryLimits {
    fn default() -> Self {
        Self {
            max_bundle_bytes: 8 * 1024 * 1024 * 1024,
            max_asset_bytes: MAX_ASSET_BYTES,
            max_roots: 128,
            max_resolved_assets: 256,
            max_cached_assets: 512,
            cache_charge_bytes: 32 * 1024 * 1024,
            sqlite_cache_kib: 512,
        }
    }
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RepositoryStats {
    pub header_queries: u64,
    pub payload_queries: u64,
    pub dependency_queries: u64,
    pub payload_bytes_read: u64,
    pub decoded_assets: u64,
    pub cache_hits: u64,
    pub cache_evictions: u64,
    pub cached_assets: usize,
    pub charged_cache_bytes: usize,
    pub peak_charged_cache_bytes: usize,
}
struct Entry {
    asset: Arc<Asset>,
    charge: usize,
    used: u64,
}

/// An operation's resolved dependency set. Private Arcs pin cache entries until drop.
/// Borrowed access prevents callers cloning a cache handle outside this lease.
pub struct AssetSet {
    publication_hash: [u8; 32],
    assets: BTreeMap<AssetId, Arc<Asset>>,
}
impl AssetSet {
    pub fn publication_hash(&self) -> [u8; 32] {
        self.publication_hash
    }
    pub fn get(&self, id: &AssetId) -> Result<&Asset> {
        self.assets
            .get(id)
            .map(Arc::as_ref)
            .ok_or_else(|| ContentError::MissingAsset(id.clone()))
    }
    pub fn item(&self, id: ItemDefinitionId) -> Result<&inventory::ItemDefinition> {
        match self.get(&AssetId::Item(id))? {
            Asset::Item(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn dialogue(&self, id: DialogueId) -> Result<&dialogue::Dialogue> {
        match self.get(&AssetId::Dialogue(id))? {
            Asset::Dialogue(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn rules(&self) -> Result<&rules::Rules> {
        match self.get(&AssetId::Rules)? {
            Asset::Rules(v) => Ok(v),
            _ => unreachable!(),
        }
    }
    pub fn len(&self) -> usize {
        self.assets.len()
    }
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }
    pub fn iter(&self) -> impl Iterator<Item = (&AssetId, &Asset)> {
        self.assets.iter().map(|(id, asset)| (id, asset.as_ref()))
    }
    fn validate(&self) -> Result<()> {
        for asset in self.assets.values() {
            match asset.as_ref() {
                Asset::Item(v) => self.rules()?.validate_mechanics(&v.mechanics)?,
                Asset::Actor(v) => v.validate(self.rules()?)?,
                Asset::Condition { value, .. } => self.validate_condition(value)?,
                Asset::Predicate(v) => self.validate_condition(&v.condition)?,
                Asset::Trigger(v) => {
                    self.validate_condition(&v.condition)?;
                    for step in &v.steps {
                        if let gameplay::SequenceStep::Apply(actions) = step {
                            for action in actions {
                                self.validate_action(action)?;
                            }
                        }
                    }
                }
                Asset::Profile(v) => {
                    for rule in &v.rules {
                        self.validate_condition(&rule.condition)?;
                    }
                }
                Asset::Dialogue(graph) => {
                    require(
                        matches!(self.get(&AssetId::DialogueContract(graph.id))?,Asset::DialogueContract(c) if c==&graph.contract()),
                        "dialogue contract mismatch",
                    )?;
                    graph.validate_messages(&|m| match self
                        .get(&AssetId::Text(m.resource))
                        .map_err(|e| game_types::Invalid(e.to_string()))?
                    {
                        Asset::Text(c) => c
                            .messages
                            .get(&m.key)
                            .ok_or_else(|| game_types::Invalid("unknown dialogue message".into())),
                        _ => unreachable!(),
                    })?;
                    for (_, args) in graph.messages() {
                        for source in args.values() {
                            if let dialogue::ArgumentSource::Attribute { attribute, .. } = source {
                                self.rules()?.attribute(attribute)?;
                            }
                        }
                    }
                }
                Asset::Action { value, .. } => self.validate_action(value)?,
                _ => {}
            }
        }
        Ok(())
    }
    fn validate_condition(&self, condition: &gameplay::Condition) -> Result<()> {
        // Local quantity/role checks and indexed dependencies were validated on read.
        // Resolve named trees and validate semantic references within this lease.
        condition.visit_resolved(
            &|id| match self.get(&AssetId::Predicate(id)) {
                Ok(Asset::Predicate(p)) => Ok(&p.condition),
                Err(e) => Err(gameplay::GameplayError::Runtime(e.to_string())),
                _ => unreachable!(),
            },
            &mut |c| {
                let result: Result<()> = (|| {
                    match c {
                        gameplay::Condition::History {
                            dialogue, event, ..
                        } => {
                            if let Asset::DialogueContract(c) =
                                self.get(&AssetId::DialogueContract(*dialogue))?
                            {
                                match event {
                                    dialogue::HistoryEvent::Line(id) => {
                                        require(c.lines.contains(id), "unknown history line")?
                                    }
                                    dialogue::HistoryEvent::Choice(id) => {
                                        require(c.choices.contains(id), "unknown history choice")?
                                    }
                                    _ => {}
                                }
                            }
                        }
                        gameplay::Condition::SkillExperience { skill, .. } => {
                            self.rules()?.skill(skill)?;
                        }
                        gameplay::Condition::ObjectiveCompleted { quest, objective } => {
                            if let Asset::Quest(q) = self.get(&AssetId::Quest(*quest))? {
                                q.objective(objective)?;
                            }
                        }
                        _ => {}
                    }
                    Ok(())
                })();
                result.map_err(|e| gameplay::GameplayError::Runtime(e.to_string()))
            },
        )?;
        Ok(())
    }
    fn validate_action(&self, action: &gameplay::Action) -> Result<()> {
        use gameplay::Action;
        match action {
            Action::Claim { actions, .. } => {
                for action in actions {
                    self.validate_action(action)?;
                }
            }
            Action::Quest {
                quest,
                transition: quests::Transition::CompleteObjective(objective),
            } => {
                if let Asset::Quest(q) = self.get(&AssetId::Quest(*quest))? {
                    q.objective(objective)?;
                }
            }
            Action::AwardExperience { skill, .. } => {
                self.rules()?.skill(skill)?;
            }
            Action::SkillCheck {
                skill,
                success,
                failure,
                ..
            } => {
                self.rules()?.skill(skill)?;
                for child in success.iter().chain(failure) {
                    self.validate_action(child)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

pub struct ContentRepository {
    pub(crate) connection: Connection,
    manifest: BundleManifest,
    limits: RepositoryLimits,
    cache: BTreeMap<AssetId, Entry>,
    stats: RepositoryStats,
    clock: u64,
}
impl ContentRepository {
    pub fn open(path: impl AsRef<Path>, limits: RepositoryLimits) -> Result<Self> {
        require(
            limits.max_asset_bytes > 0
                && limits.max_asset_bytes <= MAX_ASSET_BYTES
                && limits.max_roots > 0
                && limits.max_roots <= 4096
                && limits.max_resolved_assets > 0
                && limits.max_resolved_assets <= 4096
                && limits.max_cached_assets > 0
                && limits.max_cached_assets <= 8192
                && limits.cache_charge_bytes >= 4096
                && limits.sqlite_cache_kib > 0
                && limits.sqlite_cache_kib <= 65536,
            "invalid repository budgets",
        )?;
        let meta = std::fs::metadata(&path)?;
        require(
            meta.is_file() && meta.len() <= limits.max_bundle_bytes,
            "bundle file exceeds budget or is not a regular file",
        )?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute_batch(
            "PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; PRAGMA mmap_size=0; BEGIN;",
        )?;
        connection.pragma_update(None, "cache_size", -i64::from(limits.sqlite_cache_kib))?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        if application != BUNDLE_APPLICATION_ID {
            return Err(ContentError::WrongDatabase);
        }
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != BUNDLE_SCHEMA_VERSION {
            return Err(ContentError::BundleVersion(version));
        }
        let bytes: Vec<u8> = connection.query_row(
            "SELECT CASE WHEN length(payload)<=65536 THEN payload END FROM bundle_manifest WHERE singleton=1", [], |r| r.get(0))?;
        let manifest: BundleManifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        // Preparing these constant statements checks the expected table/column contract without scanning it.
        connection.prepare(
            "SELECT kind,id,position,byte_len,hash,payload FROM assets WHERE kind=?1 AND id=?2",
        )?;
        connection.prepare("SELECT dependency FROM asset_dependencies WHERE kind=?1 AND id=?2 ORDER BY dependency LIMIT ?3")?;
        Ok(Self {
            connection,
            manifest,
            limits,
            cache: BTreeMap::new(),
            stats: RepositoryStats::default(),
            clock: 0,
        })
    }
    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }
    pub fn stats(&self) -> RepositoryStats {
        self.stats
    }

    /// Bounded keyset pagination. Returns headers only, with no asset decoding.
    /// Pass the last header's id.key() as `after` for the next page.
    pub fn list(
        &mut self,
        kind: AssetKind,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AssetHeader>> {
        require(
            (1..=128).contains(&limit),
            "header page limit must be 1..128",
        )?;
        if let Some(key) = after {
            AssetId::from_key(kind, key.into())?;
        }
        self.stats.header_queries += 1;
        let mut stmt = self.connection.prepare(
            "SELECT id,position,byte_len,hash FROM assets WHERE kind=?1 AND id>?2 ORDER BY id LIMIT ?3")?;
        let mut rows = stmt.query(params![kind as i64, after.unwrap_or(""), limit as i64])?;
        let mut headers = Vec::new();
        while let Some(row) = rows.next()? {
            let key = row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
            let id = AssetId::from_key(kind, key.into())?;
            headers.push(read_header(row, id, 1)?);
        }
        Ok(headers)
    }

    /// Resolve a bounded batch and its mechanical dependencies. No locale is implicitly loaded.
    /// A missing/corrupt/over-budget dependency returns no partial set. Existing leases stay valid.
    pub fn load(&mut self, roots: &[AssetId]) -> Result<AssetSet> {
        budget(roots.len() <= self.limits.max_roots, "root count")?;
        let mut discovered = BTreeSet::new();
        let mut queue = VecDeque::new();
        for root in roots {
            AssetId::from_key(root.kind(), root.key())?;
            if discovered.insert(root.clone()) {
                queue.push_back(root.clone());
            }
        }
        budget(
            discovered.len() <= self.limits.max_resolved_assets,
            "dependency closure",
        )?;
        let mut set = AssetSet {
            publication_hash: self.manifest.publication_hash,
            assets: BTreeMap::new(),
        };
        while let Some(id) = queue.pop_front() {
            let asset = self.fetch(&id)?;
            for dep in asset.dependencies()? {
                if discovered.insert(dep.clone()) {
                    budget(
                        discovered.len() <= self.limits.max_resolved_assets,
                        "dependency closure",
                    )?;
                    queue.push_back(dep);
                }
            }
            set.assets.insert(id, asset);
        }
        set.validate()?;
        Ok(set)
    }

    pub(crate) fn fetch(&mut self, id: &AssetId) -> Result<Arc<Asset>> {
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.cache.get_mut(id) {
            entry.used = self.clock;
            self.stats.cache_hits += 1;
            return Ok(Arc::clone(&entry.asset));
        }
        self.stats.header_queries += 1;
        let header = self
            .connection
            .query_row(
                "SELECT position,byte_len,hash FROM assets WHERE kind=?1 AND id=?2",
                params![id.kind() as i64, id.key()],
                |r| {
                    Ok((
                        r.get::<_, u32>(0)?,
                        r.get::<_, u32>(1)? as usize,
                        blob_hash(r, 2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| ContentError::MissingAsset(id.clone()))?;
        budget(header.1 <= self.limits.max_asset_bytes, "asset bytes")?;
        let charge = header
            .1
            .checked_mul(16)
            .and_then(|n| n.checked_add(4096))
            .ok_or_else(|| corrupt(id, "asset size overflow"))?;
        self.make_room(charge)?;
        self.stats.payload_queries += 1;
        let payload: Option<Vec<u8>> = self.connection.query_row(
            "SELECT CASE WHEN length(payload)=byte_len AND byte_len<=?3 THEN payload END FROM assets WHERE kind=?1 AND id=?2",
            params![id.kind() as i64, id.key(), self.limits.max_asset_bytes as i64], |r| r.get(0))?;
        let payload = payload.ok_or_else(|| corrupt(id, "payload length mismatch"))?;
        self.stats.payload_bytes_read += payload.len() as u64;
        if blake3::hash(&payload).as_bytes() != &header.2 {
            return Err(corrupt(id, "payload checksum mismatch"));
        }
        let asset: Asset =
            serde_json::from_slice(&payload).map_err(|e| corrupt(id, e.to_string()))?;
        if asset.id() != *id {
            return Err(corrupt(id, "payload identity mismatch"));
        }
        asset
            .validate_local()
            .map_err(|e| corrupt(id, e.to_string()))?;
        self.stats.decoded_assets += 1;
        let expected = asset.dependencies()?;
        self.stats.dependency_queries += 1;
        let actual = {
            let mut stmt = self.connection.prepare(
                "SELECT dependency,target_kind,target_id FROM asset_dependencies WHERE kind=?1 AND id=?2 ORDER BY dependency LIMIT ?3")?;
            let mut rows = stmt.query(params![
                id.kind() as i64,
                id.key(),
                (MAX_ASSET_DEPENDENCIES + 1) as i64
            ])?;
            let mut deps = BTreeSet::new();
            while let Some(row) = rows.next()? {
                let bytes = row.get_ref(0)?.as_str().map_err(rusqlite::Error::from)?;
                if bytes.len() > 512 {
                    return Err(corrupt(id, "oversized dependency identity"));
                }
                let dep: AssetId =
                    serde_json::from_str(bytes).map_err(|e| corrupt(id, e.to_string()))?;
                let target_kind: i64 = row.get(1)?;
                let target_key = row.get_ref(2)?.as_str().map_err(rusqlite::Error::from)?;
                if target_kind != dep.kind() as i64 || target_key != dep.key() || !deps.insert(dep)
                {
                    return Err(corrupt(id, "dependency target mismatch or duplicate"));
                }
                if deps.len() > MAX_ASSET_DEPENDENCIES {
                    return Err(corrupt(id, "dependency count exceeded"));
                }
            }
            deps
        };
        if actual != expected {
            return Err(corrupt(id, "dependency index mismatch"));
        }
        let asset = Arc::new(asset);
        self.cache.insert(
            id.clone(),
            Entry {
                asset: Arc::clone(&asset),
                charge,
                used: self.clock,
            },
        );
        self.stats.charged_cache_bytes += charge;
        self.stats.cached_assets = self.cache.len();
        self.stats.peak_charged_cache_bytes = self
            .stats
            .peak_charged_cache_bytes
            .max(self.stats.charged_cache_bytes);
        Ok(asset)
    }
    fn make_room(&mut self, charge: usize) -> Result<()> {
        budget(
            charge <= self.limits.cache_charge_bytes,
            "cache bytes for one asset",
        )?;
        while self.cache.len() >= self.limits.max_cached_assets
            || self.stats.charged_cache_bytes > self.limits.cache_charge_bytes - charge
        {
            let victim = self
                .cache
                .iter()
                .filter(|(_, e)| Arc::strong_count(&e.asset) == 1)
                .min_by_key(|(_, e)| e.used)
                .map(|(id, _)| id.clone());
            let Some(victim) = victim else {
                return Err(ContentError::Budget(
                    "cache pinned by active asset sets".into(),
                ));
            };
            let removed = self.cache.remove(&victim).expect("selected cache entry");
            self.stats.charged_cache_bytes -= removed.charge;
            self.stats.cache_evictions += 1;
            self.stats.cached_assets = self.cache.len();
        }
        Ok(())
    }
}
fn budget(ok: bool, name: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(ContentError::Budget(name.into()))
    }
}
fn read_header(row: &rusqlite::Row<'_>, id: AssetId, start: usize) -> Result<AssetHeader> {
    Ok(AssetHeader {
        id,
        position: row.get(start)?,
        payload_bytes: row.get::<_, u32>(start + 1)? as usize,
        hash: blob_hash(row, start + 2)?,
    })
}
fn blob_hash(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<[u8; 32]> {
    row.get_ref(index)?
        .as_blob()?
        .try_into()
        .map_err(|_| rusqlite::Error::InvalidQuery)
}
