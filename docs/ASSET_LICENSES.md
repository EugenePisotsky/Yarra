# Asset licences

What has to be settled about bought assets before a build leaves this machine. Bought files and
everything made from them stay in `assets/local/`, out of Git; a skin file or preset in the repository
holds only coordinates and settings.

## To settle before shipping

| Asset | Used for | Holds | Needed |
| --- | --- | --- | --- |
| [Texturing.XYZ Female 30s Multichannel Face #07](https://texturing.xyz/collections/multi-channel-faces/products/female-30s-multichannel-face-07) | the head's skin (`content/characters/skins/face07.json`, `assets/local/characters/fitted_body/skin/head_*`) | **Personal use** licence ($19.90) | **Commercial licence** ($139.90 when last checked). Its page defines neither tier and does not mention games: ask hello@texturing.xyz about a shipped game, seats and projects, modified and downscaled maps, and the likeness of the person scanned. |
| [CG Eye Pack](https://superhivemarket.com/products/cg-eye-pack-textures--3d-model-pack) | iris and sclera (`skin/eye_*`) | [Superhive Royalty Free](https://superhivemarket.com/page/royalty-free-license) | Terms allow commercial use and forbid reselling or redistributing the product; they do not mention games or files a player can extract. Pack the textures before release; confirm with support@superhivemarket.com. |
| [Eyebrow Pack 02, Lambrador3D](https://superhivemarket.com/products/eyebrow-pack-02) | brow masks (`skin/brow_*`) | Superhive Royalty Free | As above. Its page calls the set game-ready. |
| [12 Real-time Woman Hairstyles collection 12, CGTrader](https://www.cgtrader.com/3d-models/character/woman/12-real-time-woman-hairstyles-collection-12) (`~/Downloads/hairstyles`, `hair1.rar` to `hair12.rar`; built in `assets/local/characters/hairs/`) | hair cards and textures | Bought on CGTrader under its [Royalty Free licence](https://www.cgtrader.com/pages/terms-and-conditions) (buyer confirmed) | Those terms allow games when the asset sits in a proprietary format and is shown during play, with "all commercially reasonable measures" so players cannot get at it, such as a protected resource file or encryption (§21A.2–3); no resale or redistribution outside a product (§21A.1, §21A.6). So: pack before release. Also ask the seller where the assets came from. The files carry signs of a game capture, not proof of one: an empty `Assets/Materials/ff16` entry in the Marmoset archive, and `F:\model_sell\Hair\Female hair 12\frame_6\000001DDC9784A70.dds` as a texture path in `All+12+Female+hairs.fbx`, which is how GPU capture tools name files. A marketplace licence covers only what the seller had the right to sell. The base mesh archive also holds `Nike Air Force 1 panda.jpg`, a trademark; we use neither that nor the pack's body. |
| Bought body and animations (`assets/local/characters/female_main/`) | the player, and the clips retargeted onto every MPFB character | Not recorded here | Which packs and which licences: `assets/packs/characters/female_main.toml` says only that the glb keeps both source packs' requirements. Find the receipts and fill this in. |
| Megascans surfaces (`~/Dev/Assets/terrain/sources`, `assets/local/terrain/baltic/`) | terrain textures | Not recorded here | Which account and licence they came under. |

## Settled

| Asset | Licence | Notes |
| --- | --- | --- |
| MakeHuman system assets and asset packs (`content/characters/mpfb.toml`) | CC0 | Each pack's manifest gives the licence per asset. |
| MPFB | GPL-3.0-or-later | The add-on runs in Blender and stays in `.tools/blender`, out of the game; its licence claims nothing over exports (`content/characters/README.md`). |

## Rule for a release

Loose glTF, PNG and KTX2 files can be copied straight out of a build, which the licences above either
forbid or do not address. Bought content ships packed, whichever way the answers above come back.
