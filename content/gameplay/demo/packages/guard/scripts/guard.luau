-- What happens when the player hands the gate key back to the guard.
local guard = {}

function guard.take_key(game: Game, scene: Scene)
    -- The same reward can be reached through the escort at the gate; whichever way comes
    -- first gives it, and the other finds it given.
    if game.get("guard/rewarded") then
        return
    end
    game.set("guard/rewarded", true)
    game.consume_item(scene.player, "old_gate_key", 1)
    game.complete_objective("guard/gate", "deliver")
    game.complete_quest("guard/gate")
    game.adjust_relationship(scene.speaker, scene.player, 25)
    game.award_experience(120)
    game.set_locked("guard/old_gate", false)
end

return guard
