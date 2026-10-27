"""MPFB's services for the scripts Blender runs here.

Blender loads an extension under a package path known only at run time, so a service is found by
the end of its module's name.
"""
import importlib
import sys


def service(module, name):
    suffix = f"mpfb.services.{module}"

    for loaded in list(sys.modules):
        if loaded.endswith(suffix):
            return getattr(importlib.import_module(loaded), name)

    raise RuntimeError(f"MPFB is not loaded: no module ends in {suffix}")
