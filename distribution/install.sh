#!/bin/sh
set -eu

repo="https://github.com/xavialyra/tflow/releases/latest/download"
install_dir="${HOME}/.local/bin"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

case "$(uname -m)" in
    x86_64|amd64) asset="tflow-linux-x86_64" ;;
    *)
        echo "tflow provides a Linux x86_64 build for this release." >&2
        exit 1
        ;;
esac

curl -fL "${repo}/${asset}" -o "${tmp_dir}/tflow"
mkdir -p "$install_dir"
install -m 755 "${tmp_dir}/tflow" "${install_dir}/tflow"
printf 'Installed tflow to %s\n' "${install_dir}/tflow"

case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *) printf 'Add %s to PATH to run tflow from any directory.\n' "$install_dir" ;;
esac
