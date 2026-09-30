//! Runtime composition: indexed content and disk-backed sessions, independent of authored sources.
use crate::*;
use game_types::{AreaId, Invalid, OwnerId, TriggerId, require};
use gameplay::{ContentIdentity, ContentRequest, ContentSource, GameContent, GameDefinitions};
use gameplay::{actors, inventory};
use rusqlite::{OptionalExtension, params};
use std::collections::BTreeSet;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub type RuntimeSession = save::StoredSession<ContentRepository>;
pub type ToolSession = save::StoredSession<gameplay::ToolContent>;
impl ContentSource for ContentRepository {
    fn identity(&self) -> ContentIdentity {
        ContentIdentity {
            manifest: self.manifest().content.clone(),
            fingerprint: self.manifest().mechanical_fingerprint,
        }
    }
    fn areas_at(&mut self, position: &actors::Position) -> gameplay::Result<BTreeSet<AreaId>> {
        let result = (|| -> Result<BTreeSet<AreaId>> {
            let [x, y, z] = position.millimetres;
            let mut stmt = self.connection.prepare("SELECT id FROM area_bounds INDEXED BY areas_by_x WHERE min_x<=?1 AND max_x>?1 AND min_y<=?2 AND max_y>?2 AND min_z<=?3 AND max_z>?3 ORDER BY id LIMIT 33")?;
            let mut ids = BTreeSet::new();
            for id in stmt.query_map(params![x, y, z], |r| r.get::<_, String>(0))? {
                ids.insert(AreaId::try_from(id?)?);
            }
            require(
                ids.len() <= gameplay::MAX_AREA_OVERLAP,
                "area overlap budget exceeded",
            )?;
            Ok(ids)
        })();
        result.map_err(|e| gameplay::GameplayError::Runtime(e.to_string()))
    }
    fn next_trigger(
        &mut self,
        signal: &gameplay::WorldSignal,
        after: Option<TriggerId>,
    ) -> gameplay::Result<Option<TriggerId>> {
        let result = (|| -> Result<Option<TriggerId>> {
            let id: Option<String> = self.connection.query_row("SELECT trigger FROM trigger_subscriptions WHERE signal=?1 AND trigger>?2 ORDER BY trigger LIMIT 1", params![serde_json::to_string(signal)?, after.map(|id| id.to_string()).unwrap_or_default()], |r| r.get(0)).optional()?;
            Ok(id.map(TriggerId::try_from).transpose()?)
        })();
        result.map_err(|e| gameplay::GameplayError::Runtime(e.to_string()))
    }
    fn resolve(&mut self, request: &ContentRequest) -> gameplay::Result<GameContent> {
        self.resolve_runtime(request)
            .map_err(|e| gameplay::GameplayError::Runtime(e.to_string()))
    }
}
impl ContentRepository {
    fn resolve_runtime(&mut self, request: &ContentRequest) -> Result<GameContent> {
        let roots: Vec<_> = std::iter::once(AssetId::Rules)
            .chain(
                request
                    .dialogue_contracts
                    .iter()
                    .copied()
                    .map(AssetId::DialogueContract),
            )
            .chain(request.objects.iter().copied().map(AssetId::Object))
            .chain(request.areas.iter().copied().map(AssetId::Area))
            .chain(request.triggers.iter().copied().map(AssetId::Trigger))
            .chain(request.claims.iter().copied().map(AssetId::Claim))
            .chain(request.quests.iter().copied().map(AssetId::Quest))
            .chain(request.profiles.iter().copied().map(AssetId::Profile))
            .chain(request.predicates.iter().copied().map(AssetId::Predicate))
            .chain(request.items.iter().copied().map(AssetId::Item))
            .chain(request.actors.iter().copied().map(AssetId::Actor))
            .chain(request.dialogues.iter().copied().map(AssetId::Dialogue))
            .chain(request.facts.iter().cloned().map(AssetId::Fact))
            .collect();
        let assets = self.load(&roots)?;
        // This bounded operation copy is separate from the repository's pinned cache budget.
        let mut content = GameContent {
            text: vec![],
            manifest: self.manifest().content.clone(),
            items: inventory::ItemCatalog {
                id: self.manifest().catalog_id,
                revision: self.manifest().catalog_revision,
                categories: vec![],
                items: vec![],
            },
            game: GameDefinitions {
                world: Default::default(),
                dialogue_contracts: vec![],
                claims: vec![],
                quests: vec![],
                profiles: vec![],
                predicates: vec![],
                rules: assets.rules()?.clone(),
                actors: vec![],
                dialogues: vec![],
                facts: Default::default(),
                conditions: Default::default(),
                actions: Default::default(),
            },
        };
        for (_, asset) in assets.iter() {
            match asset {
                Asset::Object(v) => content.game.world.objects.push(v.clone()),
                Asset::Area(v) => content.game.world.areas.push(v.clone()),
                Asset::Trigger(v) => content.game.world.triggers.push(v.clone()),
                Asset::Text(v) => content.text.push(v.clone()),
                Asset::DialogueContract(v) => content.game.dialogue_contracts.push(v.clone()),
                Asset::Claim(v) => content.game.claims.push(v.clone()),
                Asset::Quest(v) => content.game.quests.push(v.clone()),
                Asset::Profile(v) => content.game.profiles.push(v.clone()),
                Asset::Predicate(v) => content.game.predicates.push(v.clone()),
                Asset::Category(v) => content.items.categories.push(v.clone()),
                Asset::Item(v) => content.items.items.push(v.clone()),
                Asset::Actor(v) => content.game.actors.push(v.clone()),
                Asset::Dialogue(v) => content.game.dialogues.push(v.clone()),
                Asset::Fact(v) => {
                    content.game.facts.insert(v.clone());
                }
                Asset::Condition { id, value } => {
                    content.game.conditions.insert(id.clone(), value.clone());
                }
                Asset::Action { id, value } => {
                    content.game.actions.insert(id.clone(), value.clone());
                }
                _ => {}
            }
        }
        content.validate()?;
        for d in &content.game.world.triggers {
            let mut stmt = self.connection.prepare("SELECT signal FROM trigger_subscriptions WHERE trigger=?1 ORDER BY signal LIMIT 65")?;
            let mut indexed = BTreeSet::new();
            for row in stmt.query_map([d.id.to_string()], |r| r.get::<_, String>(0))? {
                indexed.insert(serde_json::from_str::<gameplay::WorldSignal>(&row?)?);
            }
            require(
                indexed == d.subscriptions(&content)?,
                "trigger subscription index mismatch",
            )?;
        }
        for a in &content.game.world.areas {
            let indexed: [i64; 6] = self.connection.query_row(
                "SELECT min_x,max_x,min_y,max_y,min_z,max_z FROM area_bounds WHERE id=?1",
                [a.id.to_string()],
                |r| {
                    Ok([
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ])
                },
            )?;
            let lo = a.min.millimetres;
            let hi = a.max.millimetres;
            require(
                indexed == [lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]],
                "area bounds index mismatch",
            )?;
        }
        Ok(content)
    }
}
/// Retains immutable mechanical generations independently of slots and authored source files.
/// No automatic deletion: a generation remains available to every slot that references it.
pub struct ContentLibrary {
    root: PathBuf,
    limits: RepositoryLimits,
}
impl ContentLibrary {
    pub fn new(root: impl AsRef<Path>, limits: RepositoryLimits) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().into(),
            limits,
        })
    }
    fn path(&self, id: &ContentIdentity) -> PathBuf {
        let hash: String = id.fingerprint.iter().map(|b| format!("{b:02x}")).collect();
        self.root.join(format!("{hash}.sqlite"))
    }
    /// Uses SQLite snapshot copying, so retaining a generation never copies a live WAL unsafely.
    /// Publication validation belongs to the builder; runtime readers validate requested assets.
    pub fn retain(&self, bundle: impl AsRef<Path>) -> Result<ContentIdentity> {
        let repository = ContentRepository::open(bundle, self.limits.clone())?;
        let id = repository.identity();
        let target = self.path(&id);
        if target.try_exists()? {
            self.open(&id)?;
            return Ok(id);
        }
        let stage = self.root.join(format!(".retain-{}.sqlite", OwnerId::new()));
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stage)?;
        let result = (|| {
            let mut destination = rusqlite::Connection::open(&stage)?;
            destination.execute_batch(
                "PRAGMA synchronous=FULL; PRAGMA cache_size=-512; PRAGMA mmap_size=0;",
            )?;
            {
                let backup =
                    rusqlite::backup::Backup::new(&repository.connection, &mut destination)?;
                loop {
                    match backup.step(64)? {
                        rusqlite::backup::StepResult::Done => break,
                        rusqlite::backup::StepResult::More => {}
                        _ => return Err(Invalid("content retention source busy".into()).into()),
                    }
                }
            }
            destination.pragma_update(None, "journal_mode", "DELETE")?;
            destination.close().map_err(|(_, e)| e)?;
            fs::File::open(&stage)?.sync_all()?;
            fs::hard_link(&stage, &target)?;
            fs::File::open(&self.root)?.sync_all()?;
            Ok(id)
        })();
        let _ = fs::remove_file(stage);
        result
    }
    pub fn open(&self, id: &ContentIdentity) -> Result<ContentRepository> {
        let repository = ContentRepository::open(self.path(id), self.limits.clone())?;
        require(
            repository.identity() == *id,
            "retained content identity mismatch",
        )?;
        Ok(repository)
    }
    pub fn load(
        &self,
        saves: &save::SaveDirectory,
        slot: save::SaveSlot,
        working_path: impl AsRef<Path>,
    ) -> Result<RuntimeSession> {
        let identity = saves.content_identity(slot)?;
        Ok(saves.load(slot, self.open(&identity)?, working_path)?)
    }
}
