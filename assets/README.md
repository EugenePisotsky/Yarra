# Runtime and local evaluation assets

`assets/generated/` contains derived runtime SQLite generations produced by
`yarra-world-cook`. It is ignored because it can be rebuilt from `content/`.

`assets/local/` is intentionally ignored. The legacy Forest Tree Starter Kit
does not record its author, source URL, or redistribution license, so its files
must not enter the public repository until that provenance is recovered.

The streamed demo expects the converted summer tree at:

```text
assets/local/forest_tree_07/tree_07_summer.gltf
```

This is the legacy conversion of `DA_Forest_Tree_11364_Tris.FBX`: LOD1, 4,747
triangles, approximately 15.7 m tall, with bark and alpha-cutout leaf materials.
