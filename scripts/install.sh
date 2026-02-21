#!/usr/bin/env sh
set -eu

REPO="${DEFI_REPO:-ggonzalez94/defi-cli}"
BIN_NAME="defi"
INSTALL_DIR="${DEFI_INSTALL_DIR:-/usr/local/bin}"
REQUESTED_VERSION="${DEFI_VERSION:-latest}"

fail() {
	echo "install error: $*" >&2
	exit 1
}

need_cmd() {
	command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"
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
	if [ "$REQUESTED_VERSION" = "latest" ]; then
		tag="$(
			curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" | awk -F\" '/"tag_name":/ {print $4; exit}'
		)"
		[ -n "$tag" ] || fail "could not resolve latest release tag for $REPO"
		echo "$tag"
		return
	fi

	case "$REQUESTED_VERSION" in
	v*) echo "$REQUESTED_VERSION" ;;
	*) echo "v$REQUESTED_VERSION" ;;
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

need_cmd curl
need_cmd tar
need_cmd mktemp

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

cleanup() {
	rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

echo "Installing $BIN_NAME $TAG for $OS/$ARCH from $REPO..."
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

install_without_sudo() {
	mkdir -p "$INSTALL_DIR"
	cp "$BIN_PATH" "$INSTALL_DIR/$BIN_NAME"
}

if install_without_sudo 2>/dev/null; then
	:
elif command -v sudo >/dev/null 2>&1; then
	sudo mkdir -p "$INSTALL_DIR"
	sudo cp "$BIN_PATH" "$INSTALL_DIR/$BIN_NAME"
else
	INSTALL_DIR="${HOME}/.local/bin"
	mkdir -p "$INSTALL_DIR"
	cp "$BIN_PATH" "$INSTALL_DIR/$BIN_NAME"
	echo "No write access to /usr/local/bin. Installed to $INSTALL_DIR instead."
fi

echo "Installed to $INSTALL_DIR/$BIN_NAME"
"$INSTALL_DIR/$BIN_NAME" version --long || true

case ":$PATH:" in
*":$INSTALL_DIR:"*) ;;
*)
	echo "Note: $INSTALL_DIR is not in PATH."
	echo "Add this to your shell profile:"
	echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
	;;
esac
