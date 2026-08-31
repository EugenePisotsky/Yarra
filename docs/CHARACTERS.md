# Character and animation foundation specification

This document is the contract for playable characters, party members, NPCs, and their animation
content. It describes both the small runtime foundation implemented now and the extension points
reserved for crouching, actions, equipment, and combat. Reserved concepts are requirements on future
work, not systems that should be built speculatively.

## Goals

- One movement implementation serves the player, party followers, and NPCs.
- A character instance selects presentation by stable profile ID; gameplay never branches on male,
  female, or a particular mesh name.
- Models and animation banks are separate catalog concepts even when one GLB currently contains both.
- Animation references remain stable when an exporter changes a raw GLTF clip name.
- Skeleton compatibility is explicit and validated before a graph is built.
- Standing locomotion can grow into crouched, injured, swimming, or other movement contexts without
  replacing the actor foundation.
- Equipment can later contribute movement overrides and action sets without creating a Cartesian
  catalog of every body, stance, and weapon combination.
- Visual loading and animation never become authoritative for world position, collision, camera
  focus, or streaming.

This is deliberately not a general animation framework. The current milestone does not implement
crouch input, combat, equipment, animation layers, masks, IK, retargeting, root motion, combos, or
NPC/party steering.

## Runtime ownership

An actor root owns gameplay state. Its imported model is a replaceable presentation child:

```text
intent source (input, follower steering, or AI)
  -> MoveIntent
       -> CharacterMotor + CharacterMotorConfig
            -> actor Transform + CharacterMotion
                 -> character animation presentation
                 -> camera follow       [only with CameraTarget]
                 -> world preload       [only with WorldStreamFocus]
                 -> vegetation interaction stamps [planned V2 consumer]
```

`PlayerControlled`, `CameraTarget`, and `WorldStreamFocus` are independent roles. The current main
character has all three. A future party member receives follower intent and uses the same motor, but
does not receive `CameraTarget` or `WorldStreamFocus`. An ordinary NPC receives neither role.

Movement and yaw are controller-owned. Animation observes resolved `CharacterMotion`; it cannot move
the actor root. When collision and terrain grounding arrive, they will resolve requested displacement
before it is published as `CharacterMotion`, preserving this presentation contract.

## Identities and composition

The checked-in character-presentation catalog uses stable semantic IDs and these concepts:

```text
CharacterPresentationProfile
  -> CharacterModel ---------------------> SkeletonContract
  -> movement context -> MovementSet ----> SkeletonContract
                              |
                              +-> stable AnimationClipRef
                                      -> AnimationBank -> raw GLTF clip name
```

- **Skeleton contract** is an authored compatibility ID. It states that model skinning targets and
  animation target paths are compatible. It is not runtime retargeting.
- **Character model** identifies the visual scene, its skeleton, and its normalization transform.
  Textures and materials currently belong to the complete model GLB.
- **Animation bank** identifies a GLB containing animations for one skeleton contract.
- **Animation clip** gives a raw GLTF animation a stable semantic ID and playback policy.
- **Movement set** maps the semantic Idle, Walk, and Jog roles to stable clips and owns their measured
  speeds plus controller/presentation tuning.
- **Presentation profile** composes a model with movement-set bindings. A binding is named by a
  semantic movement context such as `standing`; one binding is the default.

The profile ID, for example `presentations/female/default`, is the only selection stored on an actor
or in a future save. Choosing a male or female main character means choosing a different profile. No
movement, animation, camera, party, or save system should contain gender-specific branches.

Several meshes may share a skeleton and animation bank. A mesh with a different skeleton must use
compatible banks and movement sets. The legacy Manny/`male_rigify` material is not assumed compatible
with `yarra_humanoid_v1`; it may prove a separate profile, but a production male model must either be
bound to the canonical skeleton or ship with its own validated skeleton and animations.

## Semantic animation requests

Gameplay communicates meaning, never asset names. The eventual input to animation selection should
look conceptually like this:

```text
stance = crouched
movement = walk
combat_mode = armed
weapon_family = two_handed_sword
action = light_attack
```

It must never ask to play a raw name such as `Female_Crouch_Walk` or hold an `AnimationClip` handle.
Presentation resolves semantic state through the active profile and any equipment-provided overrides.

The current runtime implements only the profile's default movement context and the Idle/Walk/Jog
semantic request derived from `CharacterMotion`. The catalog representation permits additional
movement-context bindings, but selecting them at runtime is a later milestone.

## Future movement contexts

Standing and crouched locomotion are different movement sets selected by stance. Injured movement,
swimming, or other coherent movement modes may use the same mechanism when they are real requirements.
A movement set owns internally consistent clips and tuning; it is not a loose bag of animations.

Changing stance will eventually affect both gameplay and presentation:

- gameplay selects speed, collision shape, clearance, and allowed transitions;
- presentation selects the compatible movement set and blends poses;
- the actor root remains authoritative throughout the transition.

The first catalog contains only `standing`, so no crouch state or transition is implemented now.

## Future actions and equipment

Actions differ from continuous movement and will be modeled only when the first real action is built.
The expected composition is:

```text
base character animation profile
  + stance movement set
  + equipment animation package
      - optional movement overrides
      - action sets (attack, block, parry, reload, cast, and so on)
  + transient gameplay action request
```

An equipped weapon should contribute a package keyed by semantic weapon family. It may replace
standing/crouched movement, add actions without replacing movement, or do both. This avoids copying
every sword animation reference into every character profile.

The action design must later decide, using concrete content:

- full-body versus upper-body playback and bone masks;
- one-shot interruption, queuing, cancellation, and recovery rules;
- combo sequencing and gameplay event timing;
- additive poses and aim/look layers;
- animation notifies for footsteps, impacts, sound, and VFX;
- whether any action truly requires root motion and how collision authorizes it.

Until those decisions exist, the present system reserves stable clip IDs, skeleton compatibility,
semantic selection, and equipment override composition only. It does not add placeholder state
machines or a generic layering graph.

## Model materials and customization

For now a `CharacterModel` points to a complete GLB, including its mesh, textures, and materials. This
keeps the asset contract testable. A future profile may select a material variant for skin, hair, or
clothing when actual customization content requires it. Modular body slots and equipment meshes are
not part of this milestone and should not be inferred from the animation-set design.

## Loading and caching

The semantic catalog is compiled into the engine from
`assets/catalogs/character_presentations.catalog.ron` and validated at startup. Pack manifests remain
the source/provenance/license contract; they are not runtime animation catalogs.

Runtime caches are keyed independently:

- model ID -> loaded scene handle;
- animation-bank ID -> loaded GLTF animation source;
- movement-set ID -> prepared animation graph and semantic nodes.

Bevy may deduplicate identical underlying asset paths, but code must not depend on a single global
female scene or graph. Each actor resolves its own profile reference, while actors that select the
same definitions naturally share cached handles and graphs.

Profile replacement must replace only the visual child and presentation configuration. It must not
change the actor's stable gameplay identity or grant camera/streaming roles.

## Validation contract

Catalog startup validation rejects:

- unsupported schema versions and duplicate or malformed stable IDs;
- missing model, bank, clip, movement-set, or profile references;
- model, bank, clip, and movement-set skeleton mismatches;
- a profile whose default movement context has no binding;
- duplicate movement-context bindings within a profile;
- invalid speeds, thresholds, durations, transforms, or playback policies;
- a movement set whose required clips do not come from one compatible bank.

After the GLTF loads, graph preparation also verifies that each stable clip resolves to the authored
raw animation name. A missing local licensed asset can still fail at asset load time; it must not make
movement or camera state depend on visual readiness.

## Current implemented slice

The first slice provides:

- actor-owned direct and destination movement with Walk/Jog hysteresis, acceleration, release,
  turning, exact arrival, and playback-rate output;
- independent player, camera-target, and stream-focus roles;
- a per-actor `CharacterPresentationRef` resolved through the checked-in catalog;
- model, animation-bank, stable-clip, movement-set, and presentation-profile definitions;
- startup catalog and skeleton compatibility validation;
- caches keyed by model, bank, and movement set;
- a presentation-only visual child with Idle/Walk/Jog crossfades and Walk/Jog phase preservation;
- the current female profile as the default main-character selection.

Not implemented yet:

- a second model/profile or a runtime character-selection screen;
- non-default movement-context selection, including crouch;
- party formation/follow intent and NPC AI;
- action sets, equipment animation packages, combat, layers, masks, IK, or root motion;
- animation residency budgets or world-page ownership for character assets;
- terrain grounding, navigation, and collision.

## Local asset contract

The current runtime GLB is ignored because its source licenses do not permit treating it as a freely
redistributable repository asset. Restore it at:

```text
assets/local/characters/female_main/female_main_locomotion.glb
```

Its expected hash, bounds, source workflow, and licensing notes are recorded in
`assets/packs/characters/female_main.toml`. The semantic catalog refers to the local URI without
duplicating that provenance metadata.

## Planned increments

1. Keep the current female standing profile behavior stable through the catalog-backed resolver.
2. Add a validated second profile (likely male) to prove that model and animation selection are not
   global. It may initially use a separate skeleton and bank.
3. Add party follower intent using the existing actor motor; verify followers do not affect camera or
   stream focus.
4. Add crouch as the first non-default movement context, including gameplay collision/clearance and
   explicit transition behavior.
5. Design action playback around the first concrete interaction or combat action.
6. Add equipment-provided animation overrides alongside the first real weapon family.

Each increment should add only the abstractions exercised by its content while preserving the stable
IDs and ownership boundaries above.
