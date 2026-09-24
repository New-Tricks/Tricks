# typed: strict
# frozen_string_literal: true

# Formula for the new-tricks/homebrew-tap tap (Formula/newtricks.rb).
#   brew install --HEAD new-tricks/tap/newtricks
# Until v0.1.0 is released the formula is HEAD-only; on release, add the stable
# `url` of the tagged source tarball and its `sha256`. Submit to homebrew-core once
# the project meets its notability requirements.
class Newtricks < Formula
  desc "Design-time workbench for agent skills (search, customize, lint, publish)"
  homepage "https://github.com/new-tricks/tricks"
  license "Apache-2.0"
  head "https://github.com/new-tricks/tricks.git", branch: "main"

  depends_on "rust" => :build
  depends_on "git"

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/newtricks")
  end

  test do
    assert_match "tricks", shell_output("#{bin}/tricks --version")
    ENV["TRICKS_HOME"] = testpath
    ENV["TRICKS_CONFIG_DIR"] = testpath/"config"
    ENV["TRICKS_DATA_DIR"] = testpath/"data"
    assert_match "claude", shell_output("#{bin}/tricks --offline agents")
  end
end
