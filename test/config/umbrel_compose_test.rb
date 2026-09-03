require 'test_helper'
require 'yaml'

class UmbrelComposeTest < ActiveSupport::TestCase
  EXPECTED_ENVIRONMENT = {
    'DATABASE_PATH' => '/app/storage/production.sqlite3',
    'QUEUE_DATABASE_PATH' => '/app/storage/production_queue.sqlite3',
    'CACHE_DATABASE_PATH' => '/app/storage/production_cache.sqlite3',
    'CABLE_DATABASE_PATH' => '/app/storage/production_cable.sqlite3',
    'SECRET_KEY_BASE' => '${APP_SEED}-secret-key-base',
    'RAILS_ENV' => 'production'
  }.freeze

  # Guards against jobs broadcasting Turbo updates into CACHE_DATABASE_PATH instead of CABLE_DATABASE_PATH.
  test 'web and jobs share the exact Umbrel runtime configuration' do
    services = YAML.load_file(Rails.root.join('deltabadger/docker-compose.yml')).fetch('services')
    web = services.fetch('web')
    jobs = services.fetch('jobs')

    EXPECTED_ENVIRONMENT.each do |key, expected_value|
      { 'web' => web, 'jobs' => jobs }.each do |service_name, service|
        environment = service.fetch('environment')

        assert environment.key?(key), "#{service_name} environment is missing #{key}"
        assert_equal expected_value, environment.fetch(key),
                     "#{service_name} environment has the wrong #{key}"
      end
    end

    assert_equal web.fetch('image'), jobs.fetch('image')
    assert_equal web.fetch('volumes'), jobs.fetch('volumes')
  end
end
