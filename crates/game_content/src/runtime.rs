//! Runtime composition: published content read through SQLite, independent of authored sources.
use crate::*;
use game_types::{DialogueId, Invalid, OwnerId, require};
use gameplay::dialogue::Dialogue;
use gameplay::inventory;
use gameplay::{ContentIdentity, ContentSource, GameContent, GameDefinitions};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub type RuntimeSession = gameplay::GameSession<ContentRepository>;
pub type ToolSession = gameplay::GameSession<gameplay::ToolContent>;
/// Asset kinds read once when a session opens. Dialogue graphs and text contracts are read
/// only when a conversation or a formatter needs them.
const CORE_KINDS: [AssetKind; 15] = [
    AssetKind::Category,
    AssetKind::Item,
    AssetKind::Actor,
    AssetKind::Rules,
    AssetKind::Variable,
    AssetKind::Quest,
    AssetKind::Profile,
    AssetKind::Predicate,
    AssetKind::DialogueContract,
    AssetKind::Claim,
    AssetKind::Object,
    AssetKind::Area,
    AssetKind::Trigger,
    AssetKind::Script,
    AssetKind::Loot,
];
fn runtime_error(error: ContentError) -> gameplay::GameplayError {
    gameplay::GameplayError::Runtime(error.to_string())
}
impl ContentSource for ContentRepository {
    fn identity(&self) -> ContentIdentity {
        ContentIdentity {
            manifest: self.manifest().content.clone(),
            fingerprint: self.manifest().mechanical_fingerprint,
        }
    }
    fn core(&mut self) -> gameplay::Result<GameContent> {
        self.read_core().map_err(runtime_error)
    }
    fn dialogue(&mut self, id: DialogueId) -> gameplay::Result<Dialogue> {
        self.read_dialogue(id).map_err(runtime_error)
    }
}
impl ContentRepository {
    fn read_core(&mut self) -> Result<GameContent> {
        let Asset::Rules(rules) = self.read(&AssetId::Rules)? else {
            return Err(ContentError::MissingAsset(AssetId::Rules));
        };
        let mut content = GameContent {
            text: vec![],
            manifest: self.manifest().content.clone(),
            items: inventory::ItemCatalog {
                id: self.manifest().catalog_id,
                revision: self.manifest().catalog_revision,
                categories: vec![],
                items: vec![],
            },
            scripts: Default::default(),
            game: GameDefinitions {
                world: Default::default(),
                dialogue_contracts: vec![],
                claims: vec![],
                quests: vec![],
                profiles: vec![],
                predicates: vec![],
                rules,
                actors: vec![],
                dialogues: vec![],
                variables: vec![],
                scripts: vec![],
                loot: vec![],
            },
        };
        for kind in CORE_KINDS {
            for asset in self.read_kind(kind)? {
                match asset {
                    Asset::Object(v) => content.game.world.objects.push(v),
                    Asset::Area(v) => {
                        content.game.world.areas.insert(v);
                    }
                    Asset::Trigger(v) => content.game.world.triggers.push(v),
                    Asset::DialogueContract(v) => content.game.dialogue_contracts.push(v),
                    Asset::Claim(v) => content.game.claims.push(v),
                    Asset::Quest(v) => content.game.quests.push(v),
                    Asset::Profile(v) => content.game.profiles.push(v),
                    Asset::Predicate(v) => content.game.predicates.push(v),
                    Asset::Category(v) => content.items.categories.push(v),
                    Asset::Item(v) => content.items.items.push(v),
                    Asset::Actor(v) => content.game.actors.push(v),
                    Asset::Script(v) => content.game.scripts.push(v),
                    Asset::Loot(v) => content.game.loot.push(v),
                    Asset::Variable(v) => content.game.variables.push(v),
                    Asset::Rules(_) | Asset::Dialogue(_) | Asset::Text(_) => {}
                }
            }
        }
        content.scripts =
            scripting::LuauScripts::install(&content.game.scripts).map_err(Invalid)?;
        // Cross-references are checked here once; graphs are checked as they are loaded.
        content.validate()?;
        Ok(content)
    }
    fn read_dialogue(&mut self, id: DialogueId) -> Result<Dialogue> {
        match self.read(&AssetId::Dialogue(id))? {
            Asset::Dialogue(graph) => Ok(graph),
            _ => Err(ContentError::MissingAsset(AssetId::Dialogue(id))),
        }
    }
}
/// Retains immutable mechanical generations independently of slots and authored source files.
/// No automatic deletion: a generation remains available to every slot that references it.
pub struct ContentLibrary {
    root: PathBuf,
}
impl ContentLibrary {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().into(),
        })
    }
    fn path(&self, id: &ContentIdentity) -> PathBuf {
        let hash: String = id.fingerprint.iter().map(|b| format!("{b:02x}")).collect();
        self.root.join(format!("{hash}.sqlite"))
    }
    /// Uses SQLite snapshot copying, so retaining a generation never copies a live WAL unsafely.
    /// Publication validation belongs to the builder; runtime readers validate requested assets.
    pub fn retain(&self, bundle: impl AsRef<Path>) -> Result<ContentIdentity> {
        let repository = ContentRepository::open(bundle)?;
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
            destination.execute_batch("PRAGMA synchronous=FULL;")?;
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
        let repository = ContentRepository::open(self.path(id))?;
        require(
            repository.identity() == *id,
            "retained content identity mismatch",
        )?;
        Ok(repository)
    }
    /// Restore a slot against the exact content generation it was saved with.
    pub fn load(
        &self,
        saves: &save::SaveDirectory,
        slot: save::SaveSlot,
    ) -> Result<RuntimeSession> {
        let identity = saves.content_identity(slot)?;
        Ok(saves.load(slot, self.open(&identity)?)?)
    }
}
