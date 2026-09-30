-- The host API available to gameplay scripts. A test keeps these names equal to what the
-- engine provides. Type checking places this text in front of each script.
--
-- Identities are names as written in content: "hero", "old_gate_key", "guard/gate".

type QuestStatus = "not_started" | "active" | "completed" | "failed"

-- The pair a rule is about: the character acting and the one spoken to.
type Scene = {
    player: string,
    speaker: string,
}

-- Passed to every script. Condition scripts get only the functions of `Reads`.
type Reads = {
    item_count: (actor: string, item: string) -> number,
    quest_status: (quest: string) -> QuestStatus,
    objective_completed: (quest: string, objective: string) -> boolean,
    relationship: (from: string, to: string) -> number,
    claimed: (claim: string) -> boolean,
    present: (actor: string) -> boolean,
    -- Pass the actor whose value you mean when the variable is per actor.
    get: (variable: string, actor: string?) -> boolean | number | string,
    inside_area: (actor: string, area: string) -> boolean,
    skill_experience: (actor: string, skill: string) -> number,
    history_count: (dialogue: string, node: string) -> number,
    locked: (object: string) -> boolean,
    time: () -> number,
}

type Game = Reads & {
    give_item: (actor: string, item: string, quantity: number) -> (),
    consume_item: (actor: string, item: string, quantity: number) -> (),
    start_quest: (quest: string) -> (),
    complete_objective: (quest: string, objective: string) -> (),
    complete_quest: (quest: string) -> (),
    fail_quest: (quest: string) -> (),
    adjust_relationship: (from: string, to: string, amount: number) -> (),
    award_experience: (actor: string, skill: string, amount: number) -> (),
    set: (variable: string, value: boolean | number | string, actor: string?) -> (),
    add: (variable: string, amount: number, actor: string?) -> (),
    set_locked: (object: string, locked: boolean) -> (),
    -- Asks the engine to walk the actor into an area. Triggers hear Arrived or MoveFailed.
    move_to: (actor: string, area: string, timeout_ms: number?) -> (),
    -- Starts a conversation with the scene's player once the current command is done.
    start_dialogue: (dialogue: string, speaker: string?) -> (),
    -- A d20 check from the saved random stream. True when it passes.
    roll: (actor: string, skill: string, difficulty: number) -> boolean,
    -- Takes a once-only claim. False when it was already taken.
    claim: (claim: string) -> boolean,
}
