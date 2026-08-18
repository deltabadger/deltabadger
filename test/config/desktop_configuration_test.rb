require 'test_helper'

class DesktopConfigurationTest < ActiveSupport::TestCase
  test 'Linux builds use the empty bundle skeleton and system Ruby fallback' do
    config = JSON.parse(Rails.root.join('src-tauri/tauri.linux.conf.json').read)

    assert_equal './scripts/bundle_skeleton.sh', config.dig('build', 'beforeBuildCommand')
  end

  # A build must never start without a real updater key and the secrets that sign with it — an
  # unsigned or unverifiable artifact is worse than no artifact. The gate skips rather than fails,
  # because the same tag also builds the Docker image and an unconfigured desktop pipeline must not
  # redden an ordinary release, so what needs pinning is that the build cannot run without it.
  test 'desktop releases cannot build without a verified updater key and signing secrets' do
    workflow = Rails.root.join('.github/workflows/desktop-release.yml').read
    preflight = workflow[/^  preflight:\n(?<body>.*?)(?=^  [a-z][a-z-]*:)/m, :body]

    assert preflight, 'desktop release workflow is missing the preflight job'
    assert_includes preflight, 'REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY'
    assert_includes preflight, 'tauri signer generate'

    # Signing is mandatory (createUpdaterArtifacts is v1Compatible) and Tauri reads these with
    # var_os, so an unset secret is an empty string and takes the import path rather than skipping.
    %w[TAURI_SIGNING_PRIVATE_KEY APPLE_CERTIFICATE APPLE_ID APPLE_TEAM_ID].each do |secret|
      assert_includes preflight, secret, "preflight does not require #{secret}"
    end

    assert_match(/^  build:\n    needs: preflight\n    if: needs\.preflight\.outputs\.ready == 'true'$/,
                 workflow)
  end

  # The macOS-only calls broke the other platforms once already, in two places at once.
  test 'platform-specific window APIs stay behind their platform gate' do
    source = Rails.root.join('src-tauri/src/lib.rs').read
    macos_only = %w[ActivationPolicy title_bar_style hidden_title]

    macos_only.each do |symbol|
      source.each_line.with_index(1) do |line, number|
        next unless line.include?(symbol)

        preceding = source.lines[[number - 6, 0].max...(number - 1)].join
        assert_includes preceding, 'cfg(target_os = "macos")',
                        "#{symbol} on line #{number} is not behind a macOS gate"
      end
    end
  end
end
