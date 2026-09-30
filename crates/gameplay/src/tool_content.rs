//! Content source over a complete in-memory project, for tools, tests and authoring preview.
use crate::Result;
use crate::*;
use game_types::*;
pub struct ToolContent {
    content: GameContent,
    identity: ContentIdentity,
}
impl ToolContent {
    pub fn new(content: GameContent) -> Result<Self> {
        let identity = ContentIdentity {
            manifest: content.manifest.clone(),
            fingerprint: content.fingerprint()?,
        };
        Ok(Self { content, identity })
    }
}
impl ContentSource for ToolContent {
    fn identity(&self) -> ContentIdentity {
        self.identity.clone()
    }
    fn core(&mut self) -> Result<GameContent> {
        let mut core = self.content.clone();
        core.text.clear();
        core.game.dialogues.clear();
        Ok(core)
    }
    fn dialogue(&mut self, id: DialogueId) -> Result<dialogue::Dialogue> {
        Ok(self.content.dialogue(id)?.clone())
    }
}
