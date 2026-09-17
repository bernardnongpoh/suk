#!/bin/sh
# Sets the app version everywhere it's written. Merging the change into main releases it.
#
#   scripts/bump-version.sh 0.1.1
set -eu
version="${1:?usage: scripts/bump-version.sh X.Y.Z}"
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "Version must look like 0.1.1" >&2; exit 1; }
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
node -e '
  const fs = require("fs");
  const version = process.argv[1];
  for (const file of ["package.json", "src-tauri/tauri.conf.json"]) {
    const json = JSON.parse(fs.readFileSync(file, "utf8"));
    json.version = version;
    fs.writeFileSync(file, JSON.stringify(json, null, 2) + "\n");
  }
  const cargo = "src-tauri/Cargo.toml";
  fs.writeFileSync(cargo, fs.readFileSync(cargo, "utf8").replace(/^version = ".*"$/m, `version = "${version}"`));
' "$version"
(cd src-tauri && cargo update -p suk --offline >/dev/null 2>&1 || true)
npm install --package-lock-only --silent >/dev/null
echo "Version set to $version. Commit it and merge into main to release."
