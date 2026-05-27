require 'test_helper'
require 'yaml'

# Asserts every supported locale carries the nested translation keys that the
# starting-time rule needs. Reads YAML directly to bypass I18n::Backend::Fallbacks,
# which would otherwise let a missing locale key silently resolve through English.
class Bot::StartableLocalesTest < ActiveSupport::TestCase
  KEYS = [
    *Bot::Startable::MODES.map { |m| %W[bot settings starting_time modes options #{m}] },
    *Bot::Startable::MODES.map { |m| %W[bot settings starting_time modes display #{m}] }
  ].freeze

  test 'every available_locale has the starting_time keys in its bot.<locale>.yml' do
    I18n.available_locales.each do |locale|
      file = Rails.root.join("config/locales/bot.#{locale}.yml")
      assert File.exist?(file), "missing bot locale file for #{locale}"

      data = YAML.load_file(file)[locale.to_s]
      assert data, "bot.#{locale}.yml is missing root '#{locale}:' (malformed file?)"

      KEYS.each do |path|
        assert data.dig(*path),
               "bot.#{locale}.yml is missing #{path.join('.')}"
      end
    end
  end
end
