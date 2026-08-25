require 'test_helper'

# The Stimulus manifest is GENERATED, and a controller missing from it does not fail — it simply
# never connects. Every `data-action` pointing at it is then a line that looks wired and is not,
# which is how a segmented control moved its chip while the form it fed kept posting the old value.
#
# Cheap to check, and the failure it prevents is invisible in every other test: the views render,
# the server is fine, and only a browser knows nothing happened.
class StimulusManifestTest < ActiveSupport::TestCase
  MANIFEST = Rails.root.join('app/javascript/controllers/index.js')
  CONTROLLERS = Rails.root.join('app/javascript/controllers')

  test 'every controller is registered' do
    files = Dir.glob("#{CONTROLLERS}/**/*_controller.js").map do |path|
      Pathname.new(path).relative_path_from(CONTROLLERS).to_s.delete_suffix('.js')
    end
    manifest = MANIFEST.read
    missing = files.reject { |name| manifest.include?("./#{name}") }

    assert_empty missing,
                 "Not in app/javascript/controllers/index.js — run `bin/rails stimulus:manifest:update`: #{missing.join(', ')}"
  end

  test 'the built bundle is not older than the controllers in it' do
    bundle = Rails.root.join('app/assets/builds/application.js')
    skip 'no bundle built here' unless bundle.exist?

    newest = Dir.glob("#{CONTROLLERS}/**/*.js").map { |path| File.mtime(path) }.max
    assert_operator File.mtime(bundle), :>=, newest,
                    'app/assets/builds/application.js is stale — run `bun run build`'
  end
end
