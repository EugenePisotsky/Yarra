"""MPFB's services for the scripts Blender runs here.

Blender loads an extension under a package path known only at run time, so a service is found by
the end of its module's name.
"""
import importlib
import sys
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[3]


def service(module, name):
    return mpfb_attribute(f"mpfb.services.{module}", name)


def mpfb_attribute(suffix, name):
    """`name` from the loaded MPFB module whose path ends in `suffix`."""
    for loaded in list(sys.modules):
        if loaded.endswith(suffix):
            return getattr(importlib.import_module(loaded), name)

    raise RuntimeError(f"MPFB is not loaded: no module ends in {suffix}")


def merged(base, override):
    """A preset with a variant's changes: objects key by key, anything else replaced."""
    out = dict(base)

    for key, value in override.items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = merged(out[key], value)
        else:
            out[key] = value

    return out


def sides(name):
    """The targets a slider name stands for: an "l-" name moves both sides."""
    return [f"l-{name[2:]}", f"r-{name[2:]}"] if name.startswith("l-") else [name]


def build_human(info, subdivision=1):
    """A human from MPFB's saved-human JSON: its basemesh, its drawn parts and its eyes. A rigged
    human's parts are the rig's children, the basemesh among them. A preset's "wrap" (a file
    wrap_body.py wrote, from the repository root) moves the base mesh, and the rig and parts are
    refitted to it."""
    human = service("humanservice", "HumanService")
    objects = service("objectservice", "ObjectService")
    settings = human.get_default_deserialization_settings()
    settings["material_instances"] = "NEVER"
    settings["subdiv_levels"] = subdivision
    info = dict(info)
    wrap = info.pop("wrap", None)
    # A preset's "skin" and "eye_model" are the export's (export_glb.py).
    info.pop("skin", None)
    info.pop("eye_model", None)
    basemesh = human.deserialize_from_dict(info, settings)
    eyes = objects.find_object_of_type_amongst_nearest_relatives(basemesh, "Eyes")
    rig = basemesh.parent if basemesh.parent and basemesh.parent.type == "ARMATURE" else None
    children = [obj for obj in objects.get_list_of_children(rig or basemesh) if obj != basemesh]
    parts = [obj for obj in (basemesh, *children) if obj.type == "MESH"]

    if wrap:
        apply_wrap(basemesh, rig, parts, ROOT / wrap)

    return basemesh, parts, eyes


def apply_wrap(basemesh, rig, parts, path):
    """The wrap's offsets as a shape key over the sliders; the rig's joints and the parts then
    fitted again to the moved base mesh (its joint cubes and helpers moved with it)."""
    offsets = np.load(path)["offsets"]
    local = offsets @ np.linalg.inv(np.array(basemesh.matrix_world)[:3, :3]).T
    key = basemesh.shape_key_add(name="wrap", from_mix=False)
    positions = np.empty(len(key.data) * 3)
    key.data.foreach_get("co", positions)
    key.data.foreach_set("co", (positions.reshape(-1, 3) + local).ravel())
    key.value = 1.0

    if rig:
        service("rigservice", "RigService").refit_existing_armature(rig, basemesh)

    clothes = service("clothesservice", "ClothesService")

    for part in parts:
        if part != basemesh:
            clothes.fit_clothes_to_human(part, basemesh)
