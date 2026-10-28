"""MPFB's services for the scripts Blender runs here.

Blender loads an extension under a package path known only at run time, so a service is found by
the end of its module's name.
"""
import importlib
import sys


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


def build_human(info, subdivision=1):
    """A human from MPFB's saved-human JSON: its basemesh, its drawn parts and its eyes. A rigged
    human's parts are the rig's children, the basemesh among them."""
    human = service("humanservice", "HumanService")
    objects = service("objectservice", "ObjectService")
    settings = human.get_default_deserialization_settings()
    settings["material_instances"] = "NEVER"
    settings["subdiv_levels"] = subdivision
    basemesh = human.deserialize_from_dict(info, settings)
    eyes = objects.find_object_of_type_amongst_nearest_relatives(basemesh, "Eyes")
    rig = basemesh.parent if basemesh.parent and basemesh.parent.type == "ARMATURE" else None
    children = [obj for obj in objects.get_list_of_children(rig or basemesh) if obj != basemesh]
    parts = [obj for obj in (basemesh, *children) if obj.type == "MESH"]
    return basemesh, parts, eyes
