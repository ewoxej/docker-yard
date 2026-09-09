#!/bin/sh
set -e

REPO="ewoxej/dyard"
BIN="dyard"
INSTALL_DIR="/usr/local/bin"

latest=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\(.*\)".*/\1/')

if [ -z "$latest" ]; then
  echo "error: could not fetch latest release from $REPO" >&2
  exit 1
fi

url="https://github.com/$REPO/releases/download/$latest/dyard-linux"

echo "Installing dyard $latest -> $INSTALL_DIR/$BIN"
curl -fsSL "$url" -o "/tmp/dyard-download"
chmod +x /tmp/dyard-download

if [ -w "$INSTALL_DIR" ]; then
  mv /tmp/dyard-download "$INSTALL_DIR/$BIN"
else
  sudo mv /tmp/dyard-download "$INSTALL_DIR/$BIN"
fi

echo "Done. Run: dyard --help"
