#!/usr/bin/env bash
# The macOS installer as a Mac opens it without being talked into it: a
# universal .dmg, signed with a Developer ID and notarized by Apple.
#
# The release workflow cannot make it: the certificate lives in a keychain,
# not in the repository. Its .dmg is signed ad hoc; this one is built on a
# Mac that has the certificate.
#
# Usage:
#   scripts/build-release.sh                 build, sign, notarize: dist/
#   scripts/build-release.sh --release       the same, then add it to the
#                                            GitHub release of this version
#                                            and publish that release
#   scripts/build-release.sh --release-only  add the one already in dist/
#
# The release is the draft the workflow makes when the version's tag is
# pushed, holding the Windows and Linux installers: this adds the macOS one
# and publishes it, so a release is never seen without one of them. It is
# refused until that draft is there, and for an installer Apple has not
# notarized. Needs the GitHub CLI (`gh`), logged in.
#
# scripts/build-release.env (gitignored; copy the .example beside it) is
# sourced when it is there. Variables set before running override it:
#   SIGN_IDENTITY        the "Developer ID Application: Name (TEAMID)" string
#                        or its SHA-1 hash (default: the first one in the keychain)
#   APPLE_API_KEY_PATH   the App Store Connect API key's .p8 file,
#   APPLE_API_KEY_ID     its Key ID
#   APPLE_API_ISSUER_ID  and its Issuer ID; or, instead of the three,
#   NOTARY_PROFILE       a `notarytool store-credentials` keychain profile
#   DIST_DIR             where the installer is copied (default: dist)
#   SKIP_NOTARIZE=1      sign, but do not notarize or staple

set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [ -f scripts/build-release.env ]; then
    # shellcheck disable=SC1091
    source scripts/build-release.env
fi

SIGN_IDENTITY="${SIGN_IDENTITY:-}"
NOTARY_PROFILE="${NOTARY_PROFILE:-}"
KEY_PATH="${APPLE_API_KEY_PATH:-}"
KEY_ID="${APPLE_API_KEY_ID:-}"
ISSUER_ID="${APPLE_API_ISSUER_ID:-}"
DIST_DIR="${DIST_DIR:-dist}"
TARGET="universal-apple-darwin"

log()  { printf '\n\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "This builds the macOS installer and runs on a Mac."

BUILD=1
RELEASE=0
case "${1:-}" in
    "") ;;
    --release) RELEASE=1 ;;
    --release-only) RELEASE=1; BUILD=0 ;;
    *) die "Unknown argument '$1': --release, --release-only, or nothing." ;;
esac
[ "$#" -le 1 ] || die "One argument at most."

# The bundles are named after the workspace's version, as in the workflow
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$VERSION" ] || die "No version in Cargo.toml's [workspace.package]."

# Adds dist's installer to the version's release and publishes it
release() {
    local tag="v$VERSION" out="$1"
    command -v gh >/dev/null || die "The GitHub CLI (gh) is not installed."
    [ -f "$out" ] || die "There is no $out: run without --release-only to build it."
    xcrun stapler validate "$out" >/dev/null 2>&1 \
        || die "$out carries no notarization ticket: only a notarized installer is published."
    gh release view "$tag" >/dev/null 2>&1 \
        || die "There is no release $tag yet. Push the tag, let the Release workflow finish, then: scripts/build-release.sh --release-only"
    local missing=""
    gh release view "$tag" --json assets -q '.assets[].name' | grep -q -- '-setup\.exe$' || missing="Windows"
    [ -z "$missing" ] || die "Release $tag has no $missing installer: the workflow has not finished, or it failed."
    log "Adding $(basename "$out") to release $tag"
    gh release upload "$tag" "$out" --clobber
    gh release edit "$tag" --draft=false
    log "Published: $(gh release view "$tag" --json url -q .url)"
}

out="$DIST_DIR/N Studio_${VERSION}_universal.dmg"

# What is released is what the tag names: asked before an hour of building
if [ "$RELEASE" = 1 ] && [ "$BUILD" = 1 ]; then
    command -v gh >/dev/null || die "The GitHub CLI (gh) is not installed."
    tagged="$(git rev-list -n1 "v$VERSION" 2>/dev/null)" || die "There is no tag v$VERSION here: tag the commit and push the tag first."
    [ "$tagged" = "$(git rev-parse HEAD)" ] || die "HEAD is not the commit v$VERSION names."
    [ -z "$(git status --porcelain)" ] || die "The working tree has changes v$VERSION does not have."
fi

if [ "$BUILD" = 1 ]; then
    if [ -z "$SIGN_IDENTITY" ]; then
        SIGN_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | grep 'Developer ID Application' | head -1 \
            | sed -E 's/^[[:space:]]*[0-9]+\) [A-F0-9]+ "(.*)"$/\1/')"
    fi
    [ -n "$SIGN_IDENTITY" ] || die "No 'Developer ID Application' identity in the keychain, and SIGN_IDENTITY is not set."

    # How Apple is asked: the API key when all of it is given, else the profile
    notary=()
    if [ "${SKIP_NOTARIZE:-0}" = "1" ]; then
        warn "SKIP_NOTARIZE=1: the installer will be signed and not notarized."
    elif [ -n "$KEY_PATH" ] && [ -n "$KEY_ID" ] && [ -n "$ISSUER_ID" ]; then
        [ -f "$KEY_PATH" ] || die "APPLE_API_KEY_PATH names a file that is not there."
        notary=(--key "$KEY_PATH" --key-id "$KEY_ID" --issuer "$ISSUER_ID")
    elif [ -n "$NOTARY_PROFILE" ]; then
        xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1 \
            || die "The keychain has no notarytool profile '$NOTARY_PROFILE'."
        notary=(--keychain-profile "$NOTARY_PROFILE")
    else
        die "No notarization credentials: set the three APPLE_API_* variables or NOTARY_PROFILE (scripts/build-release.env.example), or SKIP_NOTARIZE=1."
    fi

    log "N Studio $VERSION for $TARGET"
    rustup target add aarch64-apple-darwin x86_64-apple-darwin
    (cd ui && npm ci)

    # Tauri signs the app, with the hardened runtime notarization asks for, and
    # the .dmg around it. It is given the identity alone: with the API key too
    # it would notarize the app by itself and the .dmg not at all. Apple is
    # asked once below, about the .dmg, which covers the app inside it.
    log "Building and signing"
    (cd ui && env -u APPLE_API_KEY -u APPLE_API_ISSUER -u APPLE_API_KEY_PATH -u APPLE_ID -u APPLE_PASSWORD \
        APPLE_SIGNING_IDENTITY="$SIGN_IDENTITY" npm run tauri -- build --target "$TARGET" --bundles dmg)

    bundle="target/$TARGET/release/bundle"
    app="$bundle/macos/N Studio.app"
    dmg="$bundle/dmg/N Studio_${VERSION}_universal.dmg"
    [ -f "$dmg" ] || die "The build left no $dmg."

    # What the .dmg holds is the app as it was signed; the one left in macos/
    # is checked when Tauri keeps it, and the .dmg always
    if [ -d "$app" ]; then
        codesign --verify --deep --strict "$app"
        lipo -archs "$app/Contents/MacOS/n-studio"
    fi
    codesign --force --timestamp -s "$SIGN_IDENTITY" "$dmg"
    codesign --verify --strict "$dmg"

    if [ "${#notary[@]}" -gt 0 ]; then
        log "Notarizing: this can take several minutes"
        xcrun notarytool submit "$dmg" "${notary[@]}" --wait
        xcrun stapler staple "$dmg"
        xcrun stapler validate "$dmg"
        # Gatekeeper's own answer, as on a Mac that downloaded it
        spctl -a -t open --context context:primary-signature -v "$dmg"
    fi

    mkdir -p "$DIST_DIR"
    cp "$dmg" "$out"
    log "Done: $out"
    printf '    sha256: %s\n    bytes:  %s\n' "$(shasum -a 256 "$out" | cut -d' ' -f1)" "$(stat -f%z "$out")"
fi
if [ "$RELEASE" = 1 ]; then
    release "$out"
fi
