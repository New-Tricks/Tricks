# Homebrew formula template. Publish via a tap (e.g. New-Tricks/homebrew-tap) first;
# submit to homebrew-core once the project meets its notability requirements.
# Update `url`/`sha256` from the GitHub release on each version.
class Newtricks < Formula
  desc "Design-time workbench for agent skills (search, customize, lint, publish)"
  homepage "https://github.com/New-Tricks/Tricks"
  url "https://github.com/New-Tricks/Tricks/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_TARBALL_SHA256"
  license "Apache-2.0"
  head "https://github.com/New-Tricks/Tricks.git", branch: "main"

  depends_on "rust" => :build
  depends_on "git"

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/newtricks")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tricks --version")
    ENV["TRICKS_HOME"] = testpath
    ENV["TRICKS_CONFIG_DIR"] = testpath/"config"
    ENV["TRICKS_DATA_DIR"] = testpath/"data"
    assert_match "claude", shell_output("#{bin}/tricks --offline agents")
  end
end
