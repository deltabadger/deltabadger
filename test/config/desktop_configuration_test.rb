require 'test_helper'

class DesktopConfigurationTest < ActiveSupport::TestCase
  test 'Linux builds use the empty bundle skeleton and system Ruby fallback' do
    config = JSON.parse(Rails.root.join('src-tauri/tauri.linux.conf.json').read)

    assert_equal './scripts/bundle_skeleton.sh', config.dig('build', 'beforeBuildCommand')
  end

  test 'desktop releases validate the updater public key before starting platform builds' do
    workflow = Rails.root.join('.github/workflows/desktop-release.yml').read
    validation_job = workflow[/^  validate-updater-key:\n(?<body>.*?)(?=^  [a-z][a-z-]*:)/m, :body]

    assert validation_job, 'desktop release workflow is missing the updater-key validation job'
    assert_includes validation_job, 'REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY'
    assert_includes validation_job, 'tauri signer generate'
    assert_includes validation_job, 'exit 1'
    assert_match(/^  build:\n    needs: validate-updater-key$/, workflow)
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
