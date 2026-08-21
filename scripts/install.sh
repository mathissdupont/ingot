#!/bin/sh
# Install the Ingot toolchain from a release archive.
#
#     curl -fsSL https://raw.githubusercontent.com/mathissdupont/ingot/main/scripts/install.sh | sh
#
# Or, better, read it first:
#
#     curl -fsSLO https://raw.githubusercontent.com/mathissdupont/ingot/main/scripts/install.sh
#     less install.sh && sh install.sh
#
# It exists because the alternative was `cargo install`, which needs a Rust
# toolchain and then compiles twenty-one crates on your machine — which looks
# exactly like installing twenty-one things, and is the reason this file is here.
#
# What it does, and nothing else: work out which archive fits this machine,
# download it, **verify it against the release's SHA256SUMS**, unpack three
# binaries into one directory, and say what to do next. It writes nothing outside
# that directory, needs no root, and asks nothing.
#
# Environment:
#   INGOT_VERSION   install this version rather than the newest (e.g. 0.9.0)
#   INGOT_BIN_DIR   install here rather than into the default
#   INGOT_REQUIRE_SIGNATURE=1
#                   refuse to install unless the signature over SHA256SUMS was
#                   actually verified — which needs `cosign` on this machine and
#                   a release that carries one
#
# Exit codes: 0 installed, 1 this machine is not one we ship for, 2 the install
# itself failed.

set -eu

REPO="mathissdupont/ingot"
BINARIES="ingot ingot-mcp-fs ingot-lsp"

say() { printf '%s\n' "$*"; }
oops() { printf 'install: %s\n' "$*" >&2; exit 2; }
nope() { printf 'install: %s\n' "$*" >&2; exit 1; }

# --- what this machine is --------------------------------------------------

detect_target() {
  os=$(uname -s)
  arch=$(uname -m)

  case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) nope "no release archive is built for $arch; install with \`cargo install ingot-cli\`" ;;
  esac

  case "$os" in
    Linux)
      # A gnu binary does not run on musl, and Alpine is common enough in
      # containers that failing here with the reason beats failing later with
      # "not found" from the dynamic loader.
      if [ -f /lib/ld-musl-"$arch".so.1 ] || (ldd --version 2>&1 | head -1 | grep -qi musl); then
        nope "this looks like a musl system (Alpine); the archives are glibc-linked, so install with \`cargo install ingot-cli\`"
      fi
      say_target="$arch-unknown-linux-gnu"
      suffix=".tar.gz"
      ;;
    Darwin)
      say_target="$arch-apple-darwin"
      suffix=".tar.gz"
      ;;
    *)
      nope "$os is not a platform these archives cover; on Windows use scripts/install.ps1, otherwise \`cargo install ingot-cli\`"
      ;;
  esac
}

# --- fetching --------------------------------------------------------------

if command -v curl > /dev/null 2>&1; then
  fetch() { curl -fsSL "$1" -o "$2"; }
  read_url() { curl -fsSL "$1"; }
elif command -v wget > /dev/null 2>&1; then
  fetch() { wget -qO "$2" "$1"; }
  read_url() { wget -qO - "$1"; }
else
  oops "neither curl nor wget is installed, so there is no way to download anything"
fi

# Every release here is marked pre-release, because pre-1.0 the language, the IR
# and the artifact format can still move. GitHub's `releases/latest` **excludes**
# pre-releases and answers 404, so the newest tag has to come from the list.
newest_version() {
  read_url "https://api.github.com/repos/$REPO/releases?per_page=1" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"v\{0,1\}\([^"]*\)".*/\1/p' \
    | head -1
}

# --- verifying -------------------------------------------------------------

# An unverified download is not installed. There is no flag to skip this: the
# whole reason to prefer an archive over `cargo install` is that somebody else
# built it, which is exactly why it has to be checked.
checksum_of() {
  if command -v sha256sum > /dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum > /dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    oops "no sha256sum or shasum on this machine, so the download cannot be verified; install one, or use \`cargo install ingot-cli\`"
  fi
}

# --- provenance ------------------------------------------------------------
#
# The checksum proves the download was not corrupted. It cannot prove who built
# it: `SHA256SUMS` is served by the same host as the archive, so whatever could
# replace one could replace both. A signature answers that, and it is one
# signature over `SHA256SUMS` rather than one per archive — that file is the list
# of their hashes, so covering it covers all of them.
#
# The certificate is not ours to be trusted. Keyless signing means GitHub
# attested that a particular workflow file at a particular tag ran, Sigstore
# issued a short-lived certificate saying so, and both facts are in a public log.
# What is checked below is therefore *which workflow produced this*, not *whether
# somebody still holds our key*.
#
# Checked when `cosign` is installed, and said plainly when it is not. Absence is
# not a refusal by default, because releases published before signing existed
# carry no signature at all and refusing those would mean this script cannot
# install them. `INGOT_REQUIRE_SIGNATURE=1` turns absence into a refusal, which
# is the honest way round: the strict behaviour is available and named rather
# than implied.
# Anchored at **both** ends, and that is not tidiness: cosign's
# `--certificate-identity-regexp` has to match the whole subject, so a pattern
# anchored only at the front matches nothing and refuses every signature. Found
# by running it against a real one.
SIGNATURE_IDENTITY="^https://github\.com/$REPO/\.github/workflows/release\.yml@refs/tags/v[0-9].*\$"
SIGNATURE_ISSUER="https://token.actions.githubusercontent.com"

require_signature() {
  [ -n "${INGOT_REQUIRE_SIGNATURE:-}" ]
}

# --- doing it --------------------------------------------------------------

detect_target
target="$say_target"

version="${INGOT_VERSION:-}"
if [ -z "$version" ]; then
  version=$(newest_version) || true
  [ -n "$version" ] || oops "could not ask GitHub which version is newest; set INGOT_VERSION=x.y.z and run again"
fi

name="ingot-$version-$target"
archive="$name$suffix"
base="https://github.com/$REPO/releases/download/v$version"

if [ -n "${INGOT_BIN_DIR:-}" ]; then
  bin_dir="$INGOT_BIN_DIR"
elif [ -d "$HOME/.local/bin" ]; then
  bin_dir="$HOME/.local/bin"
else
  bin_dir="$HOME/.ingot/bin"
fi

say "Ingot $version for $target"

work=$(mktemp -d 2>/dev/null || mktemp -d -t ingot)
trap 'rm -rf "$work"' EXIT INT TERM

say "  fetching  $archive"
fetch "$base/$archive" "$work/$archive" || oops "could not download $base/$archive"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || oops "could not download the checksums, so nothing was installed"

expected=$(sed -n "s/^\([0-9a-f]\{64\}\)[[:space:]][[:space:]]*$archive\$/\1/p" "$work/SHA256SUMS" | head -1)
[ -n "$expected" ] || oops "SHA256SUMS does not mention $archive, so it cannot be verified"
actual=$(checksum_of "$work/$archive")
if [ "$expected" != "$actual" ]; then
  oops "$archive does not match its checksum
  expected $expected
  got      $actual
nothing was installed"
fi
say "  verified  sha256 $actual"

# Fetched with `if` rather than `||`, because a release without a bundle is an
# ordinary case and not a failure.
if fetch "$base/SHA256SUMS.sigstore.json" "$work/SHA256SUMS.sigstore.json" 2> /dev/null; then
  if command -v cosign > /dev/null 2>&1; then
    if cosign verify-blob \
      --bundle "$work/SHA256SUMS.sigstore.json" \
      --certificate-identity-regexp "$SIGNATURE_IDENTITY" \
      --certificate-oidc-issuer "$SIGNATURE_ISSUER" \
      "$work/SHA256SUMS" > "$work/cosign.log" 2>&1
    then
      say "  verified  signed by $REPO's release workflow"
    else
      # cosign's own words, because they separate the two cases that matter: a
      # signature that does not match, and a machine that could not reach the
      # log to find out.
      cat "$work/cosign.log" >&2
      oops "the signature over SHA256SUMS did not verify, so nothing was installed"
    fi
  elif require_signature; then
    oops "INGOT_REQUIRE_SIGNATURE is set and \`cosign\` is not installed, so the signature could not be checked; nothing was installed"
  else
    say "  unchecked no \`cosign\` here, so the signature was not verified"
  fi
elif require_signature; then
  oops "v$version carries no signature over SHA256SUMS and INGOT_REQUIRE_SIGNATURE is set; nothing was installed"
else
  say "  unsigned  this release carries no signature (only the newer ones do)"
fi

tar -xzf "$work/$archive" -C "$work" || oops "could not unpack $archive"

# The two archive kinds do not agree about layout: a tarball carries a
# `ingot-<version>-<target>/` directory and the Windows zip is flat. Both are
# already published that way, so this looks for either rather than making a
# release-shaped promise depend on which one somebody downloaded.
if [ -d "$work/$name" ]; then
  root="$work/$name"
else
  root="$work"
fi

mkdir -p "$bin_dir" || oops "could not create $bin_dir"
for binary in $BINARIES; do
  [ -f "$root/$binary" ] || oops "$archive does not contain $binary"
  # Copy then move, so replacing a running binary cannot leave a half-written
  # one behind.
  cp "$root/$binary" "$bin_dir/.$binary.new" || oops "could not write into $bin_dir"
  chmod +x "$bin_dir/.$binary.new"
  mv "$bin_dir/.$binary.new" "$bin_dir/$binary"
  say "  installed $bin_dir/$binary"
done

say ""
case ":$PATH:" in
  *":$bin_dir:"*)
    say "Try it:"
    ;;
  *)
    say "$bin_dir is not on your PATH yet. Add this to your shell profile:"
    say ""
    say "    export PATH=\"$bin_dir:\$PATH\""
    say ""
    say "Then:"
    ;;
esac
say ""
say "    ingot init hello && cd hello"
say "    ingot check"
say "    ingot run --provider replay --input topic=\"compiler design\""
say ""
say "That last one produces a real artifact without contacting anything: a new"
say "project ships with a recorded fixture. \`ingot doctor\` says what a live run"
say "would still need."
