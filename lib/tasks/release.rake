# frozen_string_literal: true

# IMPORTANT: this task commits ONLY the three version files listed in
# RELEASE_VERSION_FILES. It does NOT stage anything else.
#
# Before running `release:patch|minor|major`, run `git status` and confirm
# there are no UNTRACKED files that belong in the release. New files
# (added in the working tree but never `git add`-ed) are silently left
# behind by this task, and the resulting tagged commit can ship broken
# code that references files which don't exist in the image.
#
# Past incident (v2.11.0): a model file added `include Bot::Startable`
# but the new `app/models/bot/startable.rb` was untracked when the
# release commit was made. The Docker image built fine but crashed on
# boot with `uninitialized constant Bot::Startable`, taking out every
# container the launchpad's update rolled to that version.

RELEASE_VERSION_FILES = {
  'src-tauri/Cargo.toml' => /^(version = ")[\d.]+(")/,
  'src-tauri/tauri.conf.json' => /("version": ")[\d.]+(")/,
  'deltabadger/umbrel-app.yml' => /^(version: ")[\d.]+(")/
}.freeze

namespace :release do
  desc 'Bump patch version and release (1.0.0 → 1.0.1)'
  task :patch do
    bump(:patch)
  end

  desc 'Bump minor version and release (1.0.5 → 1.1.0)'
  task :minor do
    bump(:minor)
  end

  desc 'Bump major version and release (1.2.5 → 2.0.0)'
  task :major do
    bump(:major)
  end

  def current_version
    cargo = File.read(Rails.root.join('src-tauri/Cargo.toml'))
    cargo.match(/^version = "([^"]+)"/)[1]
  end

  def next_version(type)
    major, minor, patch = current_version.split('.').map(&:to_i)

    case type
    when :major then "#{major + 1}.0.0"
    when :minor then "#{major}.#{minor + 1}.0"
    when :patch then "#{major}.#{minor}.#{patch + 1}"
    end
  end

  def bump(type)
    old_version = current_version
    new_version = next_version(type)

    puts "#{old_version} → #{new_version}"
    print 'Proceed? (y/n) '
    abort 'Aborted.' unless $stdin.gets.strip.match?(/\Ay\z/i)

    update_files(old_version, new_version)
    commit_tag_push(new_version)
    create_github_release(new_version)

    puts "\nReleased v#{new_version}"
  end

  def update_files(_old_version, new_version)
    RELEASE_VERSION_FILES.each do |file, pattern|
      path = Rails.root.join(file)
      content = File.read(path)
      content.sub!(pattern) { "#{Regexp.last_match(1)}#{new_version}#{Regexp.last_match(2)}" }
      File.write(path, content)
      puts "  updated #{file}"
    end
  end

  def commit_tag_push(version)
    files = RELEASE_VERSION_FILES.keys.join(' ')
    system("git add #{files}") || abort('git add failed')
    system("git commit -m 'Bump version to #{version}'") || abort('git commit failed')
    system('git push origin main') || abort('git push failed')
    system('git push origin main:nightly') || abort('git push to nightly failed')
    system("git tag -s v#{version} -m 'v#{version}'") || abort('git tag failed')
    system("git push origin v#{version}") || abort('git tag push failed')
  end

  def create_github_release(version)
    tag = "v#{version}"
    system("gh release create #{tag} --title #{tag} --generate-notes") || abort('gh release create failed')
  end
end
