#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 <version> <github-repository> <dist-dir>" >&2
  exit 1
fi

version="$1"
repository="$2"
dist_dir="$3"
release_base_url="https://github.com/${repository}/releases/download/v${version}"

sha256_for() {
  local archive_path="${dist_dir}/$1"

  if [[ ! -f "${archive_path}" ]]; then
    echo "missing archive ${archive_path}" >&2
    exit 1
  fi

  shasum -a 256 "${archive_path}" | awk '{print $1}'
}

darwin_arm_archive="pty-mcp-v${version}-aarch64-apple-darwin.tar.gz"
darwin_x86_archive="pty-mcp-v${version}-x86_64-apple-darwin.tar.gz"
linux_x86_archive="pty-mcp-v${version}-x86_64-unknown-linux-gnu.tar.gz"

darwin_arm_sha="$(sha256_for "${darwin_arm_archive}")"
darwin_x86_sha="$(sha256_for "${darwin_x86_archive}")"
linux_x86_sha="$(sha256_for "${linux_x86_archive}")"

cat <<EOF
class PtyMcp < Formula
  desc "MCP server for PTY management with SSH-backed remote workflows"
  homepage "https://github.com/${repository}"
  version "${version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "${release_base_url}/${darwin_arm_archive}"
      sha256 "${darwin_arm_sha}"
    else
      url "${release_base_url}/${darwin_x86_archive}"
      sha256 "${darwin_x86_sha}"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "${release_base_url}/${linux_x86_archive}"
      sha256 "${linux_x86_sha}"
    else
      odie "No prebuilt pty-mcp binary is available for this Linux CPU architecture."
    end
  end

  def install
    bin.install "pty-mcp"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/pty-mcp --version")
  end
end
EOF
