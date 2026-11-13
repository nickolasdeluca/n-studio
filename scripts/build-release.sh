#!/usr/bin/env bash
# The macOS installer as a Mac opens it without being talked into it: a
# universal .dmg, signed with a Developer ID and notarized by Apple.
#
# The release workflow cannot make it: the certificate lives in a keychain,
# not in the repository. Its .dmg is signed ad hoc; this one is built on a
# Mac that has the certificate.
#
# Usage:
#   scripts/build-release.sh
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

# The bundles are named after the workspace's version, as in the workflow
VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$VERSION" ] || die "No version in Cargo.toml's [workspace.package]."

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
cp "$dmg" "$DIST_DIR/"
out="$DIST_DIR/$(basename "$dmg")"
log "Done: $out"
printf '    sha256: %s\n    bytes:  %s\n' "$(shasum -a 256 "$out" | cut -d' ' -f1)" "$(stat -f%z "$out")"
