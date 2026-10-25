#!/usr/bin/env bash
set -euo pipefail

if [[ "${#}" -ne 2 ]]; then
  printf 'Usage: %s <tag> <output-path>\n' "${0##*/}" >&2
  exit 1
fi

TAG="${1}"
OUTPUT="${2}"

if [[ -z "${TAG}" || -z "${OUTPUT}" ]]; then
  printf 'Error: <tag> and <output-path> must be non-empty\n' >&2
  exit 1
fi

mkdir -p "$(dirname "${OUTPUT}")"

if [[ -n "${GITHUB_REPOSITORY:-}" && -n "${GITHUB_SERVER_URL:-}" ]]; then
  SERVER="${GITHUB_SERVER_URL}"
  REPO="${GITHUB_REPOSITORY}"
else
  REMOTE_URL="$(git remote get-url origin 2>/dev/null || true)"
  if [[ -z "${REMOTE_URL}" ]]; then
    printf 'Error: no GITHUB_REPOSITORY/GITHUB_SERVER_URL and no origin remote\n' >&2
    exit 1
  fi
  if [[ "${REMOTE_URL}" =~ ^https?://github\.com/([^/]+/[^/]+)\.git$ ]]; then
    REPO="${BASH_REMATCH[1]}"
    SERVER="https://github.com"
  elif [[ "${REMOTE_URL}" =~ ^git@github\.com:([^/]+/[^/]+)\.git$ ]]; then
    REPO="${BASH_REMATCH[1]}"
    SERVER="https://github.com"
  else
    printf 'Error: cannot convert origin remote %s to a GitHub server/repo\n' "${REMOTE_URL}" >&2
    exit 1
  fi
fi

if [[ "${TAG}" != v* ]]; then
  printf 'Error: tag must start with v: %s\n' "${TAG}" >&2
  exit 1
fi

if ! git rev-parse --verify --quiet "${TAG}^{commit}" >/dev/null; then
  printf 'Error: tag %s is not a commit in this repository\n' "${TAG}" >&2
  exit 1
fi

PREVIOUS=""
while IFS= read -r candidate; do
  if [[ -n "${candidate}" && "${candidate}" != "${TAG}" ]]; then
    PREVIOUS="${candidate}"
    break
  fi
done < <(git tag -l 'v*' --sort=-version:refname --merged "${TAG}")

declare -a ORDER=(
  "Breaking Changes"
  "Features"
  "Fixes"
  "Performance"
  "Refactoring"
  "Documentation"
  "Tests"
  "Build and CI"
  "Other Changes"
)

declare -A BUCKETS=()
declare -A SEEN=()

for section in "${ORDER[@]}"; do
  BUCKETS["${section}"]=""
done

if [[ -z "${PREVIOUS}" ]]; then
  # The range starts at the repository root, so an initial release covers
  # every commit reachable from the tag.
  RANGE="${TAG}"
else
  RANGE="${PREVIOUS}..${TAG}"
fi

while IFS= read -r hash; do
  subject="$(git log -1 --format='%s' "${hash}")"
  body="$(git log -1 --format='%b' "${hash}")"
  short="$(git log -1 --format='%h' "${hash}")"

  case "${subject}" in
    'chore(release):'*) continue ;;
  esac

  # Duplicates keep the newest entry, which this newest-first walk reaches
  # first.
  if [[ -n "${SEEN["${subject}"]:-}" ]]; then
    continue
  fi
  SEEN["${subject}"]=1

  section="Other Changes"
  if [[ "${subject}" == *!:* || "${body}" == *'BREAKING CHANGE:'* ]]; then
    section="Breaking Changes"
  else
    type="${subject%%:*}"
    if [[ "${subject}" == *:* ]]; then
      type="${type%%(*}"
      case "${type}" in
        feat) section="Features" ;;
        fix) section="Fixes" ;;
        perf) section="Performance" ;;
        refactor) section="Refactoring" ;;
        docs) section="Documentation" ;;
        test) section="Tests" ;;
        build | ci | chore) section="Build and CI" ;;
      esac
    fi
  fi

  BUCKETS["${section}"]+="- ${subject} ([${short}](${SERVER}/${REPO}/commit/${short}))"$'\n'
done < <(git rev-list --no-merges "${RANGE}")

{
  if [[ -z "${PREVIOUS}" ]]; then
    printf '# Initial release\n'
  else
    printf 'Full changelog: %s/%s/compare/%s...%s\n' "${SERVER}" "${REPO}" "${PREVIOUS}" "${TAG}"
  fi

  for section in "${ORDER[@]}"; do
    if [[ -z "${BUCKETS["${section}"]}" ]]; then
      continue
    fi
    printf '\n## %s\n\n' "${section}"
    printf '%s' "${BUCKETS["${section}"]}"
  done
} > "${OUTPUT}"
