-- The demo's formulas. `derive` gives every derived stat of a character from its primary
-- stats, level, class and skills; `check` decides a skill check.
local rules = {}

function rules.derive(c: Character): { [string]: number }
    return {
        ["max-health"] = 40 + c.stats.vitality * 5 + c.level * 10,
        ["max-stamina"] = 10 + c.stats.vitality,
        attack = c.stats.strength + 2 * (c.skills.swordsmanship or 0),
    }
end

-- A d20 plus two per rank and the wisdom bonus, against the difficulty.
function rules.check(c: Character, skill: string, difficulty: number, roll: (sides: number) -> number): boolean
    local rank = c.skills[skill] or 0
    return rank * 2 + (c.stats.wisdom - 10) // 2 + roll(20) >= difficulty
end

return rules
