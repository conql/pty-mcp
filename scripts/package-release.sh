#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <version> <target> <dist-dir>" >&2
  exit 1
fi

version="$1"
target="$2"
dist_dir="$3"
binary_path="target/${target}/release/pty-mcp"
archive_name="pty-mcp-v${version}-${target}.tar.gz"
archive_path="${dist_dir}/${archive_name}"
staging_dir="$(mktemp -d)"

cleanup() {
  rm -rf "${staging_dir}"
}

trap cleanup EXIT

if [[ ! -x "${binary_path}" ]]; then
  echo "expected built binary at ${binary_path}" >&2
  exit 1
fi

mkdir -p "${dist_dir}"
cp "${binary_path}" "${staging_dir}/pty-mcp"
tar -C "${staging_dir}" -czf "${archive_path}" pty-mcp
shasum -a 256 "${archive_path}"
