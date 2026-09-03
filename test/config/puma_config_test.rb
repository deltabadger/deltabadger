require 'test_helper'
require 'puma'
require 'puma/configuration'

class PumaConfigTest < ActiveSupport::TestCase
  test 'production loads without Solid Queue in Puma' do
    assert_nothing_raised { load_puma_config(solid_queue_in_puma: nil) }
  end

  test 'production loads the Solid Queue Puma plugin when enabled' do
    configuration = nil

    assert_nothing_raised do
      configuration = load_puma_config(solid_queue_in_puma: 'true')
    end

    assert_equal :async, configuration._options.file_options[:solid_queue_mode]
  end

  private

  def load_puma_config(solid_queue_in_puma:)
    original_env = ENV.slice('RAILS_ENV', 'SOLID_QUEUE_IN_PUMA')
    ENV['RAILS_ENV'] = 'production'

    if solid_queue_in_puma
      ENV['SOLID_QUEUE_IN_PUMA'] = solid_queue_in_puma
    else
      ENV.delete('SOLID_QUEUE_IN_PUMA')
    end

    configuration = Puma::Configuration.new do |user_config|
      user_config.load Rails.root.join('config/puma.rb').to_s
    end
    configuration.load
    configuration
  ensure
    ENV.delete('RAILS_ENV')
    ENV.delete('SOLID_QUEUE_IN_PUMA')
    original_env&.each { |key, value| ENV[key] = value }
  end
end
