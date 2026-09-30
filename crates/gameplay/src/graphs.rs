//! Dialogue graphs, read when a conversation first needs one. Content assembled by a tool
//! holds every graph in `game.dialogues`; a session's content reads them from its source and
//! keeps the most recently used.
use crate::Result;
use crate::dialogue::Dialogue;
use game_types::DialogueId;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Graphs kept in memory at once. An older one is read again when it is needed.
pub const MAX_LOADED_DIALOGUES: usize = 32;

type Read = dyn FnMut(DialogueId) -> Result<Dialogue>;

/// Where a content's graphs are read from, and those read lately. Not part of the content's
/// value: a clone reads from the same place but keeps its own graphs.
#[derive(Default)]
pub struct Graphs {
    read: Option<Rc<RefCell<Read>>>,
    /// Least recently used first.
    loaded: RefCell<VecDeque<Rc<Dialogue>>>,
}
impl Graphs {
    pub fn reading(read: impl FnMut(DialogueId) -> Result<Dialogue> + 'static) -> Self {
        Self {
            read: Some(Rc::new(RefCell::new(read))),
            loaded: RefCell::default(),
        }
    }
    /// The graph if it is in memory, now the most recently used.
    pub(crate) fn cached(&self, id: DialogueId) -> Option<Rc<Dialogue>> {
        let mut loaded = self.loaded.borrow_mut();
        let index = loaded.iter().position(|graph| graph.id == id)?;
        let graph = loaded.remove(index)?;
        loaded.push_back(graph.clone());
        Some(graph)
    }
    /// Reads a graph from the source, if the content has one.
    pub(crate) fn read(&self, id: DialogueId) -> Option<Result<Dialogue>> {
        let read = self.read.as_ref()?;
        Some((read.borrow_mut())(id))
    }
    /// Keeps a graph just read, letting go of the least recently used beyond the limit.
    pub(crate) fn keep(&self, graph: Dialogue) -> Rc<Dialogue> {
        let graph = Rc::new(graph);
        let mut loaded = self.loaded.borrow_mut();
        if loaded.len() == MAX_LOADED_DIALOGUES {
            loaded.pop_front();
        }
        loaded.push_back(graph.clone());
        graph
    }
    /// How many graphs are in memory.
    pub fn loaded(&self) -> usize {
        self.loaded.borrow().len()
    }
}
impl Clone for Graphs {
    fn clone(&self) -> Self {
        Self {
            read: self.read.clone(),
            loaded: RefCell::default(),
        }
    }
}
impl PartialEq for Graphs {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}
impl Eq for Graphs {}
impl std::fmt::Debug for Graphs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Graphs({} loaded)", self.loaded())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_graph_used_least_recently_goes_first() {
        let template = crate::fixtures::content().game.dialogues[0].clone();
        let graph = |n: usize| Dialogue {
            id: DialogueId::named(&format!("graph/{n}")),
            ..template.clone()
        };
        let graphs = Graphs::default();
        for n in 0..MAX_LOADED_DIALOGUES {
            graphs.keep(graph(n));
        }
        // Using the first leaves the second as the one used least recently.
        assert!(graphs.cached(graph(0).id).is_some());
        graphs.keep(graph(MAX_LOADED_DIALOGUES));
        assert_eq!(graphs.loaded(), MAX_LOADED_DIALOGUES);
        assert!(graphs.cached(graph(0).id).is_some());
        assert!(graphs.cached(graph(1).id).is_none());
        // A clone reads from the same place but starts with nothing in memory.
        assert_eq!(graphs.clone().loaded(), 0);
    }
}
