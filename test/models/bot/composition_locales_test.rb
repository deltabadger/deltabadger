require 'test_helper'
require 'yaml'

# YAML read directly, not I18n: with fallbacks on, a missing key resolves through English.
class Bot::CompositionLocalesTest < ActiveSupport::TestCase
  EVENTS = %w[orders_below_minimum].freeze

  test 'every available_locale carries the composition activity events in its own base.<locale>.yml' do
    I18n.available_locales.each do |locale|
      data = YAML.load_file(Rails.root.join("config/locales/base.#{locale}.yml"))[locale.to_s]
      EVENTS.each do |event|
        assert data.dig('bot_activity', 'events', event), "base.#{locale}.yml is missing bot_activity.events.#{event}"
      end
    end
  end
end
