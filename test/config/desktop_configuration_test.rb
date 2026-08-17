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
end
