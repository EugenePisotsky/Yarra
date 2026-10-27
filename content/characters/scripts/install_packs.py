"""Unpacks MakeHuman asset packs into MPFB's data folder; a pack MPFB finds malformed stops it.

setup_blender.py runs it: blender -b --python install_packs.py -- PACK.zip ...
"""
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from mpfb_services import service

AssetService = service("assetservice", "AssetService")
LocationService = service("locationservice", "LocationService")

data = Path(LocationService.get_user_data())

for pack in sys.argv[sys.argv.index("--") + 1 :]:
    problem = AssetService.check_asset_pack_zip(pack)

    if problem is not None:
        sys.exit(f"{pack}: {problem}")

    with zipfile.ZipFile(pack) as archive:
        archive.extractall(data)

    print(f"installed {Path(pack).name}")

print(f"PACKS_DONE {data}")
