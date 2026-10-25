#!/usr/bin/env bash
# Build, sign (Developer ID) and notarize the macOS app.
#
# Credentials are read from the environment or from a gitignored `.env.local`
# at the repo root. See README "Signing and notarization" for details.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -f .env.local ]]; then
  set -a
  # shellcheck disable=SC1091
  source .env.local
  set +a
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  APPLE_SIGNING_IDENTITY="$(security find-identity -v -p codesigning \
    | sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' | head -n 1)"
fi

if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  echo "error: no 'Developer ID Application' certificate found in the keychain." >&2
  echo "       Create one at https://developer.apple.com/account/resources/certificates" >&2
  exit 1
fi
export APPLE_SIGNING_IDENTITY

# Notarization: either an App Store Connect API key or an Apple ID + app-specific password.
if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
  echo "Notarizing with App Store Connect API key"
elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
  echo "Notarizing with Apple ID ${APPLE_ID}"
else
  echo "error: notarization credentials missing. Set either:" >&2
  echo "         APPLE_ID, APPLE_PASSWORD (app-specific password), APPLE_TEAM_ID" >&2
  echo "       or APPLE_API_ISSUER, APPLE_API_KEY, APPLE_API_KEY_PATH" >&2
  exit 1
fi

echo "Signing with: ${APPLE_SIGNING_IDENTITY}"
pnpm tauri build --bundles app,dmg

APP="src-tauri/target/release/bundle/macos/Carbon Disk Treemap.app"
echo "Verifying..."
codesign --verify --deep --strict --verbose=2 "${APP}"
spctl --assess --type execute --verbose=4 "${APP}"
xcrun stapler validate "${APP}"
