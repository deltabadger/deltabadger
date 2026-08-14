#!/usr/bin/env ruby

require 'fileutils'
require 'find'
require 'pathname'

repo_dir = Pathname.new(ARGV.fetch(0)).expand_path
app_dir = Pathname.new(ARGV.fetch(1)).expand_path
expected_app_dir = repo_dir.join('_bundle/app')
abort "refusing to replace unexpected app bundle path: #{app_dir}" unless app_dir == expected_app_dir

# Keep the application inventory in one place for both platform bundlers. New files under the
# Rails runtime directories are picked up automatically; build inputs, credentials, tests and
# generated/runtime state stay out of the shipped resources.
excluded_directories = %w[
  .bundle
  .claude
  .cursor
  .desktop-ruby-cache
  .git
  .github
  .ruby-lsp
  .vscode
  _bundle
  coverage
  docs
  graphify-out
  log
  node_modules
  public/assets
  spec
  src-tauri
  storage
  test
  tmp
  vendor
].freeze

excluded_files = %w[
  config/master.key
  config/credentials.yml.enc
  migrated_legacy_accounts.txt
  not_migrated_optin.csv
].freeze

FileUtils.rm_rf(app_dir)
FileUtils.mkdir_p(app_dir)

Find.find(repo_dir.to_s) do |source_name|
  source = Pathname.new(source_name)
  relative = source.relative_path_from(repo_dir).to_s.tr('\\', '/')
  next if relative == '.'

  if source.directory? && excluded_directories.include?(relative)
    Find.prune
    next
  end

  basename = source.basename.to_s
  next if excluded_files.include?(relative)
  next if relative.start_with?('config/credentials/') && basename.end_with?('.key')
  next if basename.start_with?('.env') || basename.include?('.sqlite3')

  destination = app_dir.join(relative)
  if source.symlink?
    FileUtils.mkdir_p(destination.dirname)
    FileUtils.copy_entry(source, destination, true, false)
  elsif source.directory?
    FileUtils.mkdir_p(destination)
  elsif source.file?
    FileUtils.mkdir_p(destination.dirname)
    FileUtils.copy_file(source, destination, true)
  end
end

# Rails reads the desktop version from this file at boot. Copy only the version source rather
# than Rust sources and target artifacts.
FileUtils.mkdir_p(app_dir.join('src-tauri'))
FileUtils.copy_file(repo_dir.join('src-tauri/Cargo.toml'), app_dir.join('src-tauri/Cargo.toml'), true)

%w[log storage tmp/cache tmp/pids].each { |relative| FileUtils.mkdir_p(app_dir.join(relative)) }

abort 'bundled Rails launcher is missing' unless app_dir.join('bin/rails').file?
abort 'desktop version source is missing' unless app_dir.join('src-tauri/Cargo.toml').file?

leaked_env = Dir.glob(app_dir.join('**/.env*'), File::FNM_DOTMATCH).find { |path| File.file?(path) }
abort "an .env file was copied into the app bundle: #{leaked_env}" if leaked_env
