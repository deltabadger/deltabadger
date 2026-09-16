# frozen_string_literal: true

require 'English'

# IMPORTANT: this task commits ONLY the three version files listed in
# RELEASE_VERSION_FILES. It does NOT stage anything else.
#
# Before running `release:patch|minor|major`, run `git status` and confirm
# there are no UNTRACKED files that belong in the release. New files
# (added in the working tree but never `git add`-ed) are silently left
# behind by this task, and the resulting tagged commit can ship broken
# code that references files which don't exist in the image.
#
# This has happened (v2.11.0): a model file added `include Bot::Startable`
# but the new `app/models/bot/startable.rb` was untracked when the
# release commit was made. The Docker image built fine and then crashed on
# boot with `uninitialized constant Bot::Startable`.

# Guarded because test workers reload lib/tasks after resetting Rake.application.
unless defined?(RELEASE_VERSION_FILES)
  RELEASE_VERSION_FILES = {
    'src-tauri/Cargo.toml' => /^(version = ")[\d.]+(")/,
    'src-tauri/tauri.conf.json' => /("version": ")[\d.]+(")/,
    'deltabadger/umbrel-app.yml' => /^(version: ")[\d.]+(")/
  }.freeze
end

namespace :release do
  desc 'Bump patch version and release (1.0.0 → 1.0.1)'
  task :patch do
    bump(:patch)
  end

  desc 'Bump minor version and release (1.0.5 → 1.1.0)'
  task :minor do
    bump(:minor)
  end

  desc 'Bump major version and release (1.2.5 → 2.0.0); takes a hand-written notes.md'
  task :major do
    bump(:major, major_notes)
  end

  # A major is an announcement, not a changelog, so its prose is written by a person and passed
  # in as markdown. Rails' rake shim rejects an unknown top-level task before any task runs, so
  # the file name only survives as a bare word thanks to the no-op task defined at the bottom.
  def major_notes
    file = ARGV.grep(/\.md\z/).first
    abort 'Write the announcement first, then: rails release:major notes.md' if file.nil?
    abort "#{file}: no such file" unless File.exist?(file)

    File.expand_path(file)
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

  def bump(type, notes_file = nil)
    old_version = current_version
    new_version = next_version(type)

    puts "#{old_version} → #{new_version}"
    print 'Proceed? (y/n) '
    abort 'Aborted.' unless $stdin.gets.strip.match?(/\Ay\z/i)

    update_files(old_version, new_version)
    commit_tag_push(new_version)
    create_github_release(new_version, notes_file || summarize_changes(type))

    puts "\nReleased v#{new_version}; GitHub Actions is attaching the desktop artifacts"
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
    # Fast-forwards nightly onto the release so the :nightly image never sits BEHIND stable —
    # anyone running image_tag: "nightly" would otherwise be on older code than a stable user.
    # The cost is that one release starts three workflow runs: this push builds the nightly image,
    # the tag below builds the same commit again as :latest, and the desktop release also watches
    # v* tags. The two Docker builds are the same commit under different tags, about three minutes
    # each, and the registry dedupes the layers — cheaper than special-casing either workflow.
    system('git push origin main:nightly') || abort('git push to nightly failed')
    system("git tag -s v#{version} -m 'v#{version}'") || abort('git tag failed')
    system("git push origin v#{version}") || abort('git tag push failed')
  end

  # The release exists as soon as the tag is pushed, whatever the desktop build does.
  # desktop-release.yml adopts this release (`gh release view || gh release create`) and uploads
  # its DMG, installer and updater manifest into it once every arch has finished — so a tag meant
  # only for the Docker image still gets its notes, and a desktop build that fails or is skipped
  # costs the assets, not the release.
  def create_github_release(version, notes)
    tag = "v#{version}"
    # Argument form, not a command line: the notes path comes from whatever the operator typed,
    # and a quote in it would otherwise break the shell after the tag has already been pushed.
    flags = notes ? ['--notes-file', notes.to_s] : []
    system('gh', 'release', 'create', tag, '--title', tag, '--generate-notes', *flags) ||
      abort('gh release create failed')
  end

  # A few plain lines above GitHub's generated commit list, written by the local Claude CLI from
  # the commit subjects. Best effort: no claude on PATH, no previous tag, or a non-zero exit just
  # means the release keeps the generated notes alone.
  def summarize_changes(type)
    previous = `git describe --tags --abbrev=0 HEAD^ 2>/dev/null`.strip
    return if previous.empty?

    log = `git log #{previous}..HEAD^ --no-merges --format=%s`.strip
    return if log.empty?

    puts '  writing release notes...'
    summary = IO.popen(['claude', '-p', "#{notes_prompt(type)}\n#{log}"], in: File::NULL, &:read).to_s.strip
    return unless $CHILD_STATUS.success? && summary.present?

    Rails.root.join('tmp/release-notes.md').tap { |file| File.write(file, "#{summary}\n") }
  rescue Errno::ENOENT
    nil
  end

  # The same prose goes to the release page, Telegram and Discord, so it is written for a reader
  # scrolling a channel: a patch is one sentence saying where the fixes landed, and a minor is
  # only what a user can now see or do. Nobody subscribes to a list of internals.
  def notes_prompt(type)
    intro = 'These commits are a release of an open-source trading bot.'

    if type == :patch
      <<~PROMPT
        #{intro} Reply with ONE short sentence naming what was fixed, starting with
        "Fixes in ". No bullets, no preamble, no closing remarks.
      PROMPT
    else
      <<~PROMPT
        #{intro} Reply with 2-4 bullets, one line each, covering ONLY what a user can now
        see or do that they could not before. Skip bug fixes, refactors and internals
        entirely. Write what it does for them, not how it was built. Output the bullets and
        nothing else: no preamble, no closing remarks, no notes on what you left out. If
        nothing here is user-facing, output nothing at all.
      PROMPT
    end
  end
end

# `rails release:major notes.md`: rake reads the file name as a second task to run and Rails
# aborts on it as unrecognized before release:major ever starts. Declaring it as a no-op lets
# the word through; major_notes is what actually reads it.
ARGV.grep(/\.md\z/) { |file| task(file) }
