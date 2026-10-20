#!/bin/zsh
# Makes a world built by build_world.sh the one the game and editor open by default: its project
# and views into content/, its runtime into assets/generated/. The build folder is left with its
# intermediates only. Usage: install_world.sh BUILT_DIR [--replace]; without --replace it refuses
# to overwrite an existing default world, and with it the replaced world is moved to
# tmp/world-backups/<time>/.
setopt pipefail errexit
cd "${0:A:h}/../../.."
W=${1:?usage: install_world.sh BUILT_DIR [--replace]}
if [[ ! -f $W/BUILD_DONE || ! -f $W/world.project.sqlite || ! -f $W/world.runtime.sqlite ]]; then
  echo "$W holds no finished build (build_world.sh leaves BUILD_DONE)" >&2; exit 1
fi
if [[ -e content/world.project.sqlite && $2 != --replace ]]; then
  echo "content/world.project.sqlite exists; pass --replace to overwrite it" >&2; exit 1
fi
sqlite3 $W/world.project.sqlite "PRAGMA wal_checkpoint(TRUNCATE);" >/dev/null
old=(content/world.project.sqlite content/world.project.sqlite-shm content/world.project.sqlite-wal
  content/world.project.views assets/generated/world.runtime.sqlite)
old=(${^old}(N))
if (( $#old )); then
  backup=tmp/world-backups/$(date +%Y%m%d-%H%M%S)
  mkdir -p $backup
  mv $old $backup/
  echo "moved the replaced world to $backup"
fi
mv $W/world.project.sqlite content/world.project.sqlite
rm -f $W/world.project.sqlite-shm $W/world.project.sqlite-wal $W/BUILD_DONE
mv $W/world.project.views content/world.project.views
mkdir -p assets/generated
mv $W/world.runtime.sqlite assets/generated/world.runtime.sqlite
echo "installed $W as the default world (content/world.project.sqlite, assets/generated/world.runtime.sqlite)"
