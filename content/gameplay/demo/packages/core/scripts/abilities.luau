-- What the demo's abilities do when they take effect. `scene.player` is the character
-- using the ability and `scene.speaker` its target.
local abilities = {}

function abilities.strike(game: Game, scene: Scene)
    local damage = game.stat(scene.player, "attack") + game.random(6)
    game.change_resource(scene.speaker, "health", -damage)
end

function abilities.power_strike(game: Game, scene: Scene)
    game.change_resource(scene.speaker, "health", -2 * game.stat(scene.player, "attack"))
end

return abilities
