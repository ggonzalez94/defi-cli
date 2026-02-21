#!/usr/bin/env sh
set -eu

TARGET_VERSION="${1:-}"

REPO="${DEFI_REPO:-ggonzalez94/defi-cli}"
BIN_NAME="defi"
REQUESTED_VERSION="${DEFI_VERSION:-${TARGET_VERSION:-latest}}"
EXPLICIT_INSTALL_DIR="${DEFI_INSTALL_DIR:-}"
SYSTEM_INSTALL="${DEFI_SYSTEM_INSTALL:-0}"
ALLOW_SUDO="${DEFI_USE_SUDO:-0}"

fail() {
	echo "install error: $*" >&2
	exit 1
}

need_cmd() {
	command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
}

is_true() {
	case "$1" in
	1 | true | TRUE | yes | YES | on | ON) return 0 ;;
	*) return 1 ;;
	esac
}

detect_os() {
	os="$(uname -s | tr '[:upper:]' '[:lower:]')"
	case "$os" in
	linux) echo "linux" ;;
	darwin) echo "darwin" ;;
	*) fail "unsupported OS: $os (supported: linux, darwin)" ;;
	esac
}

detect_arch() {
	arch="$(uname -m)"
	case "$arch" in
	x86_64 | amd64) echo "amd64" ;;
	aarch64 | arm64) echo "arm64" ;;
	*) fail "unsupported architecture: $arch (supported: amd64, arm64)" ;;
	esac
}

resolve_tag() {
	if [ "$REQUESTED_VERSION" = "latest" ] || [ "$REQUESTED_VERSION" = "stable" ]; then
		tag="$(
			curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | awk -F\" '/"tag_name":/ {print $4; exit}'
		)"
		[ -n "$tag" ] || fail "could not resolve latest release tag for $REPO"
		echo "$tag"
		return
	fi

	case "$REQUESTED_VERSION" in
	v*) echo "$REQUESTED_VERSION" ;;
	[0-9]*) echo "v$REQUESTED_VERSION" ;;
	*) fail "invalid version '$REQUESTED_VERSION' (use latest, stable, vX.Y.Z, or X.Y.Z)" ;;
	esac
}

sha256_cmd() {
	if command -v sha256sum >/dev/null 2>&1; then
		echo "sha256sum"
		return
	fi
	if command -v shasum >/dev/null 2>&1; then
		echo "shasum -a 256"
		return
	fi
	fail "missing checksum tool: sha256sum or shasum"
}

path_contains() {
	case ":$PATH:" in
	*":$1:"*) return 0 ;;
	*) return 1 ;;
	esac
}

first_writable_path_dir() {
	old_ifs="$IFS"
	IFS=':'
	for dir in $PATH; do
		[ -n "$dir" ] || continue
		[ -d "$dir" ] || continue
		[ -w "$dir" ] || continue
		echo "$dir"
		IFS="$old_ifs"
		return 0
	done
	IFS="$old_ifs"
	return 1
}

resolve_install_dir() {
	if [ -n "$EXPLICIT_INSTALL_DIR" ]; then
		echo "$EXPLICIT_INSTALL_DIR"
		return
	fi

	if is_true "$SYSTEM_INSTALL"; then
		echo "/usr/local/bin"
		return
	fi

	if dir="$(first_writable_path_dir)"; then
		echo "$dir"
		return
	fi

	[ -n "${HOME:-}" ] || fail "HOME is not set and no writable PATH directory was found"
	echo "$HOME/.local/bin"
}

ensure_install_dir() {
	[ -d "$INSTALL_DIR" ] && return

	if mkdir -p "$INSTALL_DIR" 2>/dev/null; then
		return
	fi

	if is_true "$ALLOW_SUDO" && command -v sudo >/dev/null 2>&1; then
		sudo mkdir -p "$INSTALL_DIR"
		return
	fi

	fail "cannot create $INSTALL_DIR (use DEFI_INSTALL_DIR=<writable-dir> or DEFI_USE_SUDO=1)"
}

install_binary() {
	target="$INSTALL_DIR/$BIN_NAME"
	tmp_target="$target.tmp.$$"

	if cp "$BIN_PATH" "$tmp_target" 2>/dev/null && chmod 0755 "$tmp_target" 2>/dev/null && mv "$tmp_target" "$target" 2>/dev/null; then
		return
	fi
	rm -f "$tmp_target" 2>/dev/null || true

	if is_true "$ALLOW_SUDO" && command -v sudo >/dev/null 2>&1; then
		sudo cp "$BIN_PATH" "$tmp_target"
		sudo chmod 0755 "$tmp_target"
		sudo mv "$tmp_target" "$target"
		return
	fi

	fail "no write access to $INSTALL_DIR (set DEFI_INSTALL_DIR=<writable-dir> or DEFI_USE_SUDO=1)"
}

need_cmd curl
need_cmd tar
need_cmd mktemp
need_cmd awk
need_cmd grep

OS="$(detect_os)"
ARCH="$(detect_arch)"
TAG="$(resolve_tag)"
VERSION="${TAG#v}"
ARCHIVE="${BIN_NAME}_${VERSION}_${OS}_${ARCH}.tar.gz"
CHECKSUMS="checksums.txt"
BASE_URL="https://github.com/$REPO/releases/download/$TAG"
TMP_DIR="$(mktemp -d)"
ARCHIVE_PATH="$TMP_DIR/$ARCHIVE"
CHECKSUMS_PATH="$TMP_DIR/$CHECKSUMS"
BIN_PATH="$TMP_DIR/$BIN_NAME"
INSTALL_DIR="$(resolve_install_dir)"

cleanup() {
	rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

echo "Installing $BIN_NAME $TAG for $OS/$ARCH from $REPO..."
echo "Target install directory: $INSTALL_DIR"
curl -fsSL "$BASE_URL/$ARCHIVE" -o "$ARCHIVE_PATH"
curl -fsSL "$BASE_URL/$CHECKSUMS" -o "$CHECKSUMS_PATH"

expected_line="$(grep "  $ARCHIVE\$" "$CHECKSUMS_PATH" || true)"
[ -n "$expected_line" ] || fail "checksum entry for $ARCHIVE not found in $CHECKSUMS"

(
	cd "$TMP_DIR"
	echo "$expected_line" > expected.checksum
	cmd="$(sha256_cmd)"
	$cmd -c expected.checksum >/dev/null 2>&1 || fail "checksum verification failed"
)

tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR"
[ -f "$BIN_PATH" ] || fail "archive did not contain $BIN_NAME binary"
chmod +x "$BIN_PATH"

ensure_install_dir
install_binary

echo "Installed to $INSTALL_DIR/$BIN_NAME"
"$INSTALL_DIR/$BIN_NAME" version --long || true

if ! path_contains "$INSTALL_DIR"; then
	echo "Note: $INSTALL_DIR is not in PATH."
	echo "Add this to your shell profile:"
	echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi
