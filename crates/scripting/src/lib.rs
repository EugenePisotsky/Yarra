//! Luau scripts for gameplay content. A script module is a file returning a table of
//! functions; content refers to one as `module.function`. Every call gets a `game` table
//! whose functions are thin translations onto `gameplay`'s script scopes, so a script can do
//! nothing the built-in rules cannot.
//!
//! Each call loads its module afresh from bytecode: nothing a script keeps in a variable
//! survives to the next call. Calls run sandboxed, with a memory cap and a step budget.
use game_types::{Invalid, Key};
use gameplay::quests::{Status, Transition};
use gameplay::{ActScope, Action, GameplayError, Participant, ReadScope, ScriptEngine};
use gameplay::{ScriptModule, ScriptName, Scripts};
use mlua::chunk::{ChunkMode, Compiler};
use mlua::{Function, IntoLua, Lua, Scope, Table, Value, VmState};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

/// Type definitions of the host API. Prepended to a script, they let `luau-analyze` check it.
pub const API_DEFINITIONS: &str = include_str!("api.d.luau");
const MEMORY_LIMIT: usize = 16 * 1024 * 1024;
/// Interrupt checks allowed per call. Luau raises one at loop back-edges and calls.
const STEP_BUDGET: u64 = 1_000_000;

struct Module {
    bytecode: Vec<u8>,
    exports: BTreeSet<String>,
}
pub struct LuauScripts {
    lua: Lua,
    modules: BTreeMap<String, Module>,
    steps: Rc<Cell<u64>>,
}
fn host(error: GameplayError) -> mlua::Error {
    mlua::Error::runtime(error.to_string())
}
fn id<T: TryFrom<String, Error = Invalid>>(name: String) -> mlua::Result<T> {
    T::try_from(name).map_err(|e| mlua::Error::runtime(e.to_string()))
}
fn key(name: String) -> mlua::Result<Key> {
    Key::new(name).map_err(|e| mlua::Error::runtime(e.to_string()))
}
fn count(value: i64, what: &str) -> mlua::Result<u32> {
    u32::try_from(value).map_err(|_| mlua::Error::runtime(format!("{what} is out of range")))
}
/// Lets the read functions be written once for both kinds of script.
trait Reads: Copy {
    fn read<R>(self, f: impl FnOnce(&ReadScope) -> R) -> R;
}
impl Reads for &ReadScope<'_> {
    fn read<R>(self, f: impl FnOnce(&ReadScope) -> R) -> R {
        f(self)
    }
}
impl Reads for &RefCell<&mut ActScope<'_, '_>> {
    fn read<R>(self, f: impl FnOnce(&ReadScope) -> R) -> R {
        f(&self.borrow().read())
    }
}
fn add_reads<'s, T: Reads + 's>(scope: &'s Scope<'s, '_>, game: &Table, on: T) -> mlua::Result<()> {
    game.set(
        "item_count",
        scope.create_function(move |_, (actor, item): (String, String)| {
            let (actor, item) = (id(actor)?, id(item)?);
            on.read(|r| r.item_count(actor, item)).map_err(host)
        })?,
    )?;
    game.set(
        "quest_status",
        scope.create_function(move |_, quest: String| {
            let quest = id(quest)?;
            Ok(match on.read(|r| r.quest_status(quest)).map_err(host)? {
                Status::NotStarted => "not_started",
                Status::Active => "active",
                Status::Completed => "completed",
                Status::Failed => "failed",
            })
        })?,
    )?;
    game.set(
        "objective_completed",
        scope.create_function(move |_, (quest, objective): (String, String)| {
            let (quest, objective) = (id(quest)?, key(objective)?);
            on.read(|r| r.objective_completed(quest, &objective))
                .map_err(host)
        })?,
    )?;
    game.set(
        "relationship",
        scope.create_function(move |_, (from, to): (String, String)| {
            let (from, to) = (id(from)?, id(to)?);
            on.read(|r| r.relationship(from, to)).map_err(host)
        })?,
    )?;
    game.set(
        "claimed",
        scope.create_function(move |_, claim: String| {
            let claim = id(claim)?;
            on.read(|r| r.claimed(claim)).map_err(host)
        })?,
    )?;
    game.set(
        "present",
        scope.create_function(move |_, actor: String| {
            let actor = id(actor)?;
            Ok(on.read(|r| r.present(actor)))
        })?,
    )?;
    game.set(
        "get",
        scope.create_function(move |lua, (variable, actor): (String, Option<String>)| {
            let (variable, actor) = (id(variable)?, actor.map(id).transpose()?);
            match on.read(|r| r.variable(variable, actor)).map_err(host)? {
                gameplay::Value::Bool(value) => value.into_lua(lua),
                gameplay::Value::Int(value) => value.into_lua(lua),
                gameplay::Value::Text(value) => value.into_lua(lua),
            }
        })?,
    )?;
    game.set(
        "inside_area",
        scope.create_function(move |_, (actor, area): (String, String)| {
            let (actor, area) = (id(actor)?, id(area)?);
            on.read(|r| r.inside_area(actor, area)).map_err(host)
        })?,
    )?;
    game.set(
        "skill_experience",
        scope.create_function(move |_, (actor, skill): (String, String)| {
            let (actor, skill) = (id(actor)?, key(skill)?);
            on.read(|r| r.skill_experience(actor, &skill)).map_err(host)
        })?,
    )?;
    game.set(
        "history_count",
        scope.create_function(move |_, (dialogue, node): (String, String)| {
            let (dialogue, node) = (id(dialogue)?, key(node)?);
            on.read(|r| r.history_count(dialogue, &node)).map_err(host)
        })?,
    )?;
    game.set(
        "locked",
        scope.create_function(move |_, object: String| {
            let object = id(object)?;
            on.read(|r| r.locked(object)).map_err(host)
        })?,
    )?;
    game.set(
        "time",
        scope.create_function(move |_, ()| Ok(on.read(|r| r.time().0)))?,
    )?;
    Ok(())
}
fn add_effects<'s>(
    scope: &'s Scope<'s, '_>,
    game: &Table,
    on: &'s RefCell<&mut ActScope<'_, '_>>,
) -> mlua::Result<()> {
    // Most effects are the built-in actions, run for the actor the script names.
    let apply = move |actor: game_types::ActorId, action: Action| -> mlua::Result<()> {
        on.borrow_mut().apply(actor, &action).map_err(host)
    };
    let player = move || on.borrow().player;
    game.set(
        "give_item",
        scope.create_function(move |_, (actor, item, quantity): (String, String, i64)| {
            apply(
                id(actor)?,
                Action::GrantItem {
                    definition: id(item)?,
                    quantity: count(quantity, "quantity")?,
                },
            )
        })?,
    )?;
    game.set(
        "consume_item",
        scope.create_function(move |_, (actor, item, quantity): (String, String, i64)| {
            apply(
                id(actor)?,
                Action::ConsumeItem {
                    definition: id(item)?,
                    quantity: count(quantity, "quantity")?,
                },
            )
        })?,
    )?;
    let quest = move |quest: String, transition: Transition| -> mlua::Result<()> {
        apply(
            player(),
            Action::Quest {
                quest: id(quest)?,
                transition,
            },
        )
    };
    game.set(
        "start_quest",
        scope.create_function(move |_, name: String| quest(name, Transition::Start))?,
    )?;
    game.set(
        "complete_objective",
        scope.create_function(move |_, (name, objective): (String, String)| {
            quest(name, Transition::CompleteObjective(key(objective)?))
        })?,
    )?;
    game.set(
        "complete_quest",
        scope.create_function(move |_, name: String| quest(name, Transition::Complete))?,
    )?;
    game.set(
        "fail_quest",
        scope.create_function(move |_, name: String| quest(name, Transition::Fail))?,
    )?;
    game.set(
        "adjust_relationship",
        scope.create_function(move |_, (from, to, amount): (String, String, i64)| {
            let amount = i16::try_from(amount)
                .map_err(|_| mlua::Error::runtime("amount is out of range"))?;
            apply(
                player(),
                Action::Relationship {
                    from: Participant::Actor(id(from)?),
                    to: Participant::Actor(id(to)?),
                    amount,
                },
            )
        })?,
    )?;
    game.set(
        "award_experience",
        scope.create_function(move |_, (actor, skill, amount): (String, String, i64)| {
            apply(
                id(actor)?,
                Action::AwardExperience {
                    skill: key(skill)?,
                    amount: u64::from(count(amount, "amount")?),
                },
            )
        })?,
    )?;
    game.set(
        "set",
        scope.create_function(
            move |_, (variable, value, actor): (String, Value, Option<String>)| {
                let value = match value {
                    Value::Boolean(value) => gameplay::Value::Bool(value),
                    Value::Integer(value) => gameplay::Value::Int(value),
                    Value::Number(value) if value.fract() == 0.0 && value.abs() < 9e15 => {
                        gameplay::Value::Int(value as i64)
                    }
                    Value::String(value) => gameplay::Value::Text(value.to_str()?.to_owned()),
                    _ => {
                        return Err(mlua::Error::runtime(
                            "a variable holds true/false, a whole number or text",
                        ));
                    }
                };
                apply(
                    player(),
                    Action::Set {
                        variable: id(variable)?,
                        of: actor.map(id).transpose()?.map(Participant::Actor),
                        value,
                    },
                )
            },
        )?,
    )?;
    game.set(
        "add",
        scope.create_function(
            move |_, (variable, amount, actor): (String, i64, Option<String>)| {
                apply(
                    player(),
                    Action::Add {
                        variable: id(variable)?,
                        of: actor.map(id).transpose()?.map(Participant::Actor),
                        amount,
                    },
                )
            },
        )?,
    )?;
    game.set(
        "set_locked",
        scope.create_function(move |_, (object, locked): (String, bool)| {
            apply(
                player(),
                Action::SetLocked {
                    object: id(object)?,
                    locked,
                },
            )
        })?,
    )?;
    game.set(
        "move_to",
        scope.create_function(
            move |_, (actor, area, timeout_ms): (String, String, Option<i64>)| {
                let timeout_ms = timeout_ms
                    .map(u64::try_from)
                    .transpose()
                    .map_err(|_| mlua::Error::runtime("timeout is out of range"))?;
                apply(
                    player(),
                    Action::Move {
                        actor: Participant::Actor(id(actor)?),
                        to: id(area)?,
                        timeout_ms,
                    },
                )
            },
        )?,
    )?;
    game.set(
        "start_dialogue",
        scope.create_function(move |_, (dialogue, speaker): (String, Option<String>)| {
            apply(
                player(),
                Action::StartDialogue {
                    dialogue: id(dialogue)?,
                    speaker: speaker.map(id).transpose()?.map(Participant::Actor),
                },
            )
        })?,
    )?;
    game.set(
        "roll",
        scope.create_function(
            move |_, (actor, skill, difficulty): (String, String, i64)| {
                let (actor, skill) = (id(actor)?, key(skill)?);
                let difficulty = count(difficulty, "difficulty")?;
                on.borrow_mut()
                    .roll(actor, &skill, difficulty)
                    .map_err(host)
            },
        )?,
    )?;
    game.set(
        "claim",
        scope.create_function(move |_, claim: String| {
            let claim = id(claim)?;
            on.borrow_mut().claim(claim).map_err(host)
        })?,
    )?;
    Ok(())
}
impl LuauScripts {
    /// Compiles every module and records what each exports. A module is a chunk returning
    /// a table of functions; anything else is an authoring error reported here.
    pub fn new(modules: &[ScriptModule]) -> Result<Self, String> {
        let lua = Lua::new();
        // Chance and time come from the host, so a replay after loading behaves the same.
        (|| -> mlua::Result<()> {
            let math: Table = lua.globals().get("math")?;
            math.set("random", Value::Nil)?;
            math.set("randomseed", Value::Nil)?;
            let os: Table = lua.globals().get("os")?;
            for name in ["time", "clock", "date", "difftime"] {
                os.set(name, Value::Nil)?;
            }
            Ok(())
        })()
        .map_err(|e| e.to_string())?;
        lua.sandbox(true).map_err(|e| e.to_string())?;
        lua.set_memory_limit(MEMORY_LIMIT)
            .map_err(|e| e.to_string())?;
        let steps = Rc::new(Cell::new(STEP_BUDGET));
        let remaining = steps.clone();
        lua.set_interrupt(move |_| {
            let left = remaining.get();
            if left == 0 {
                return Err(mlua::Error::runtime("script exceeded its step budget"));
            }
            remaining.set(left - 1);
            Ok(VmState::Continue)
        });
        let mut engine = Self {
            lua,
            modules: BTreeMap::new(),
            steps,
        };
        for module in modules {
            let name = module.name.as_str().to_owned();
            let bytecode = Compiler::new()
                .compile(&module.source)
                .map_err(|e| format!("script {name}: {e}"))?;
            engine.modules.insert(
                name.clone(),
                Module {
                    bytecode,
                    exports: BTreeSet::new(),
                },
            );
            let table = engine
                .load(&name)
                .map_err(|e| format!("script {name}: {e}"))?;
            let mut exports = BTreeSet::new();
            for pair in table.pairs::<String, Value>() {
                let (export, value) = pair.map_err(|e| format!("script {name}: {e}"))?;
                if !matches!(value, Value::Function(_)) {
                    return Err(format!("script {name}: export {export} is not a function"));
                }
                exports.insert(export);
            }
            engine
                .modules
                .get_mut(&name)
                .expect("just inserted")
                .exports = exports;
        }
        Ok(engine)
    }
    /// The engine as content carries it.
    pub fn install(modules: &[ScriptModule]) -> Result<Scripts, String> {
        Ok(Scripts::new(Rc::new(Self::new(modules)?)))
    }
    fn load(&self, module: &str) -> mlua::Result<Table> {
        let compiled = self
            .modules
            .get(module)
            .ok_or_else(|| mlua::Error::runtime(format!("unknown script module {module}")))?;
        self.steps.set(STEP_BUDGET);
        match self
            .lua
            .load(&compiled.bytecode[..])
            .set_name(format!("={module}"))
            .set_mode(ChunkMode::Binary)
            .eval::<Value>()?
        {
            Value::Table(table) => Ok(table),
            _ => Err(mlua::Error::runtime(
                "a script must return a table of functions",
            )),
        }
    }
    fn function(&self, name: &ScriptName) -> mlua::Result<Function> {
        self.load(name.module.as_str())?.get(name.function.as_str())
    }
    fn scene(&self, read: &ReadScope) -> mlua::Result<Table> {
        let scene = self.lua.create_table()?;
        scene.set("player", read.player.to_string())?;
        scene.set("speaker", read.speaker.to_string())?;
        Ok(scene)
    }
}
fn failed(name: &ScriptName, error: mlua::Error) -> GameplayError {
    Invalid(format!("script {name}: {error}")).into()
}
impl ScriptEngine for LuauScripts {
    fn exports(&self, name: &ScriptName) -> bool {
        self.modules
            .get(name.module.as_str())
            .is_some_and(|m| m.exports.contains(name.function.as_str()))
    }
    fn condition(&self, name: &ScriptName, read: &ReadScope) -> gameplay::Result<bool> {
        let result = self.lua.scope(|scope| {
            let game = self.lua.create_table()?;
            add_reads(scope, &game, read)?;
            match self
                .function(name)?
                .call::<Value>((game, self.scene(read)?))?
            {
                Value::Boolean(value) => Ok(value),
                _ => Err(mlua::Error::runtime(
                    "a condition must return true or false",
                )),
            }
        });
        result.map_err(|e| failed(name, e))
    }
    fn action(&self, name: &ScriptName, act: &mut ActScope) -> gameplay::Result<()> {
        let scene = self.scene(&act.read()).map_err(|e| failed(name, e))?;
        let act = RefCell::new(act);
        let result = self.lua.scope(|scope| {
            let game = self.lua.create_table()?;
            add_reads(scope, &game, &act)?;
            add_effects(scope, &game, &act)?;
            self.function(name)?.call::<()>((game, scene))
        });
        result.map_err(|e| failed(name, e))
    }
}
