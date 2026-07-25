#!/usr/bin/env bash
set -euo pipefail

export GIT_PAGER=cat
export PAGER=cat

crate_name="tuitodo"

usage() {
    cat <<'EOF'
Usage: ./scripts/release.sh [major|minor|patch]

Bump Tuido (default: minor), validate, commit, tag, and publish to crates.io.
Latest stable Tuicore is selected from crates.io. This script never pushes.
EOF
}

confirm() {
    local prompt="$1"
    local answer

    [[ -t 0 ]] || return 1
    read -r -p "$prompt [y/N] " answer || return 1
    [[ "$answer" =~ ^[Yy]([Ee][Ss])?$ ]]
}

crates_io_version_state() {
    local crate_name="$1"
    local version="$2"

    python3 - "$crate_name" "$version" <<'PY'
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

crate_name, expected = sys.argv[1:]
url = "https://crates.io/api/v1/crates/{}/{}".format(
    urllib.parse.quote(crate_name, safe=""),
    urllib.parse.quote(expected, safe=""),
)
request = urllib.request.Request(
    url,
    headers={"User-Agent": "tuido-release/1.0 (https://github.com/daure/tuido)"},
)
try:
    with urllib.request.urlopen(request, timeout=10) as response:
        payload = json.load(response)
except urllib.error.HTTPError as error:
    if error.code == 404:
        print("absent")
        raise SystemExit(0)
    raise SystemExit(f"error: crates.io version query failed with HTTP {error.code}")
except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
    raise SystemExit(f"error: crates.io version query failed: {error}")

version = payload.get("version") if isinstance(payload, dict) else None
if not isinstance(version, dict) or version.get("num") != expected:
    raise SystemExit("error: invalid crates.io version response")
print("present")
PY
}

remote_tag_must_be_absent() {
    local tag="$1"
    local status

    if git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1; then
        printf 'error: tag %s already exists on origin\n' "$tag" >&2
        return 1
    else
        status=$?
    fi
    if (( status != 2 )); then
        printf 'error: failed to verify tag %s on origin\n' "$tag" >&2
        return 1
    fi
}

case "${1:-minor}" in
    -h|--help)
        usage
        exit 0
        ;;
    major|minor|patch)
        bump="${1:-minor}"
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac

if (( $# > 1 )); then
    usage >&2
    exit 2
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
cd "$repo_root"

for command in git cargo python3; do
    if ! command -v "$command" >/dev/null 2>&1; then
        printf 'error: required command not found: %s\n' "$command" >&2
        exit 1
    fi
done

if [[ -n "$(git status --porcelain --untracked-files=all)" ]]; then
    printf 'error: Git working tree must be clean, including untracked files\n' >&2
    exit 1
fi

if ! branch="$(git symbolic-ref --quiet --short HEAD)"; then
    printf 'error: cannot release from detached HEAD\n' >&2
    exit 1
fi

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
cargo_token="${CARGO_REGISTRIES_CRATES_IO_TOKEN:-${CARGO_REGISTRY_TOKEN:-}}"
if [[ -z "$cargo_token" ]]; then
    cargo_token="$(python3 - "$cargo_home/credentials.toml" "$cargo_home/credentials" <<'PY'
import pathlib
import sys
import tomllib

for raw_path in sys.argv[1:]:
    path = pathlib.Path(raw_path)
    if not path.is_file():
        continue
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError):
        continue
    candidates = [
        data.get("registries", {}).get("crates-io", {}).get("token"),
        data.get("registry", {}).get("token"),
    ]
    for token in candidates:
        if isinstance(token, str) and token.strip():
            print(token.strip())
            raise SystemExit(0)
raise SystemExit(1)
PY
    )" || true
fi

if [[ -z "$cargo_token" ]]; then
    printf 'error: cannot resolve crates.io token; use `cargo login` or CARGO_REGISTRY_TOKEN\n' >&2
    exit 1
fi
unset cargo_token

latest_tuicore="$(python3 <<'PY'
import json
import re
import urllib.error
import urllib.request

request = urllib.request.Request(
    "https://crates.io/api/v1/crates/tuicore",
    headers={"User-Agent": "tuido-release/1.0 (https://github.com/daure/tuido)"},
)
try:
    with urllib.request.urlopen(request, timeout=10) as response:
        payload = json.load(response)
except (OSError, urllib.error.URLError, json.JSONDecodeError) as error:
    raise SystemExit(f"error: failed to query crates.io for Tuicore: {error}")

versions = payload.get("versions") if isinstance(payload, dict) else None
if not isinstance(versions, list):
    raise SystemExit("error: invalid crates.io response: missing versions list")

stable = []
for version in versions:
    if not isinstance(version, dict) or not isinstance(version.get("num"), str):
        raise SystemExit("error: invalid crates.io response: malformed version entry")
    match = re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\+[0-9A-Za-z.-]+)?", version["num"])
    if match and version.get("yanked") is False:
        stable.append((tuple(map(int, match.groups())), version["num"]))

if not stable:
    raise SystemExit("error: crates.io has no stable non-yanked Tuicore version")
print(max(stable)[1])
PY
)"

read -r old_version new_version declared_tuicore < <(python3 - "$bump" "$latest_tuicore" <<'PY'
import pathlib
import re
import sys

bump, latest = sys.argv[1:]
text = pathlib.Path("Cargo.toml").read_text()
package = re.search(r'(?ms)^\[package\]\s*$\n(?P<body>.*?)(?=^\[|\Z)', text)
dependencies = re.search(r'(?ms)^\[dependencies\]\s*$\n(?P<body>.*?)(?=^\[|\Z)', text)
if package is None or dependencies is None:
    raise SystemExit("error: Cargo.toml must have [package] and [dependencies] sections")

package_version = re.search(r'(?m)^\s*version\s*=\s*["\']([^"\']+)["\']', package.group("body"))
tuicore = re.search(r'(?m)^\s*tuicore\s*=\s*["\']([^"\']+)["\']\s*$', dependencies.group("body"))
if package_version is None or tuicore is None:
    raise SystemExit("error: package version and Tuicore registry dependency must use string versions")

def strict_version(label, value):
    if re.fullmatch(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", value) is None:
        raise SystemExit(f"error: {label} version must be X.Y.Z, found {value!r}")
    return tuple(map(int, value.split(".")))

old = package_version.group(1)
major, minor, patch = strict_version("Tuido", old)
declared = tuicore.group(1)
declared_parts = strict_version("declared Tuicore", declared)
latest_parts = strict_version("latest Tuicore", latest.split("+", 1)[0])
if declared_parts > latest_parts:
    raise SystemExit(f"error: declared Tuicore {declared} is newer than crates.io {latest}")

if bump == "major":
    new = (major + 1, 0, 0)
elif bump == "minor":
    new = (major, minor + 1, 0)
else:
    new = (major, minor, patch + 1)
print(old, ".".join(map(str, new)), declared)
PY
)

tag="v$new_version"
if git rev-parse --quiet --verify "refs/tags/$tag" >/dev/null; then
    printf 'error: tag %s already exists\n' "$tag" >&2
    exit 1
fi
remote_tag_must_be_absent "$tag"
target_version_state="$(crates_io_version_state "$crate_name" "$new_version")"
if [[ "$target_version_state" != "absent" ]]; then
    printf 'error: %s %s already exists on crates.io\n' "$crate_name" "$new_version" >&2
    exit 1
fi

mutated=false
committed=false
tagged=false
failure_guidance() {
    status=$?
    if (( status != 0 )) && [[ "$mutated" == true ]]; then
        printf '\nRelease stopped; inspect with: git --no-pager status && git --no-pager diff\n' >&2
        if [[ "$committed" == true ]]; then
            printf 'Release commit remains. Inspect with: git --no-pager show --stat HEAD\n' >&2
        fi
        if [[ "$tagged" == true ]]; then
            printf 'Release tag remains. Verify clean tree, tag identity, and crates.io absence before resuming.\n' >&2
        fi
        printf 'Do not rerun bump or rewrite history automatically.\n' >&2
    fi
    exit "$status"
}
trap failure_guidance EXIT

mutated=true
if [[ "$declared_tuicore" != "$latest_tuicore" ]]; then
    python3 - "$declared_tuicore" "$latest_tuicore" <<'PY'
import pathlib
import re
import sys

old, new = sys.argv[1:]
path = pathlib.Path("Cargo.toml")
text = path.read_text()
updated, count = re.subn(
    r'(?m)^(\s*tuicore\s*=\s*["\'])' + re.escape(old) + r'(["\']\s*)$',
    rf'\g<1>{new}\g<2>',
    text,
    count=1,
)
if count != 1:
    raise SystemExit("error: Tuicore dependency changed unexpectedly")
path.write_text(updated)
PY
fi
cargo update -p tuicore --precise "$latest_tuicore"

python3 - "$old_version" "$new_version" <<'PY'
import pathlib
import re
import sys

old, new = sys.argv[1:]
manifest_path = pathlib.Path("Cargo.toml")
manifest = manifest_path.read_text()
package = re.search(r'(?ms)^\[package\]\s*$\n(?P<body>.*?)(?=^\[|\Z)', manifest)
body = package.group("body")
updated_body, count = re.subn(
    r'(?m)^(\s*version\s*=\s*["\'])' + re.escape(old) + r'(["\'])',
    rf'\g<1>{new}\g<2>',
    body,
    count=1,
)
if count != 1:
    raise SystemExit("error: package version changed unexpectedly")
manifest_path.write_text(manifest[:package.start("body")] + updated_body + manifest[package.end("body"):])

lock_path = pathlib.Path("Cargo.lock")
lock = lock_path.read_text()
blocks = list(re.finditer(r'(?ms)^\[\[package\]\]\s*$\n.*?(?=^\[\[package\]\]|\Z)', lock))
matches = []
for block in blocks:
    name = re.search(r'(?m)^name\s*=\s*["\']([^"\']+)["\']', block.group())
    version = re.search(r'(?m)^version\s*=\s*["\']([^"\']+)["\']', block.group())
    if name and version and name.group(1) == "tuido" and version.group(1) == old:
        matches.append((block, version))
if len(matches) != 1:
    raise SystemExit(f"error: expected one Tuido {old} lock entry, found {len(matches)}")
block, version = matches[0]
start = block.start() + version.start(1)
end = block.start() + version.end(1)
lock_path.write_text(lock[:start] + new + lock[end:])
PY

cargo test --locked
cargo package --locked --allow-dirty --registry crates-io
cargo publish --locked --allow-dirty --dry-run --registry crates-io

printf '\nTuido: %s -> %s\n' "$old_version" "$new_version"
printf 'Tuicore: %s -> %s (latest crates.io)\n' "$declared_tuicore" "$latest_tuicore"
git --no-pager diff -- Cargo.toml Cargo.lock
if ! confirm "Commit, tag, and publish Tuido $new_version with Tuicore $latest_tuicore?"; then
    printf 'Release canceled; dependency/version changes remain in working tree.\n' >&2
    exit 1
fi

git add Cargo.toml Cargo.lock
git commit -m "release: $tag"
committed=true
git tag -a "$tag" -m "release: $tag"
tagged=true

if [[ -n "$(git status --porcelain --untracked-files=all)" ]]; then
    printf 'error: Git working tree must be clean before publishing\n' >&2
    exit 1
fi
if [[ "$(git rev-parse HEAD)" != "$(git rev-parse "$tag^{commit}")" ]]; then
    printf 'error: HEAD must match release tag %s before publishing\n' "$tag" >&2
    exit 1
fi
target_version_state="$(crates_io_version_state "$crate_name" "$new_version")"
if [[ "$target_version_state" != "absent" ]]; then
    printf 'error: %s %s appeared on crates.io before publish\n' "$crate_name" "$new_version" >&2
    exit 1
fi

if ! cargo publish --locked --registry crates-io; then
    publish_state="$(crates_io_version_state "$crate_name" "$new_version")"
    if [[ "$publish_state" == "present" ]]; then
        printf '\n%s %s is present on crates.io; publication succeeded despite cargo error.\n' "$crate_name" "$new_version"
    else
        printf '\nPublish failed; release commit and tag %s remain.\n' "$tag" >&2
        printf 'Resume only after: tree is clean; HEAD equals %s; %s %s is still absent on crates.io.\n' "$tag" "$crate_name" "$new_version" >&2
        printf 'Then run: cargo publish --locked --registry crates-io\n' >&2
        trap - EXIT
        exit 1
    fi
fi

trap - EXIT
printf '\nPublished %s. Push release commit and tag when ready:\n' "$tag"
printf 'git push origin %s\n' "$branch"
printf 'git push origin %s\n' "$tag"
