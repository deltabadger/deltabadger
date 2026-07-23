ENV['RAILS_ENV'] ||= 'test'

# The suite's baseline is a self-hosted container: no platform market data.
# Tests that exercise the hosted path opt in by setting MARKET_DATA_URL themselves
# and clear it again afterwards. A developer with these exported in their shell
# (handy for pointing `rails server` at a real data-api) would otherwise flip the
# whole suite into hosted mode and fail every stock/market-data assertion.
# Same intent as WebMock.disable_net_connect! below: cut the suite off from ambient config.
%w[MARKET_DATA_URL MARKET_DATA_TOKEN MARKET_DATA_PROVIDER_NAME].each { |key| ENV.delete(key) }

require_relative '../config/environment'
require 'rails/test_help'
require 'mocha/minitest'

# WebMock eagerly checks every available HTTP adapter. The app currently bundles
# curb 1.3.x indirectly, which is newer than WebMock's stated tested range and
# produces a warning on every test boot even though these tests use Faraday/net-http.
class WebMockRequireStderrFilter
  def initialize(stderr)
    @stderr = stderr
  end

  def write(message)
    return if message.match?(/WebMock is known to work with Curb/)

    @stderr.write(message)
  end

  def flush
    @stderr.flush
  end
end

begin
  original_stderr = $stderr
  $stderr = WebMockRequireStderrFilter.new(original_stderr)
  require 'webmock/minitest'
ensure
  $stderr = original_stderr if original_stderr
end

require_relative 'support/exchange_mock_helpers'

WebMock.disable_net_connect!(allow_localhost: true)

puts "\n\e[1mDeltabadger v#{Rails.application.config.version}\e[0m\n\n"

module ActiveSupport
  class TestCase
    include FactoryBot::Syntax::Methods
    include ExchangeMockHelpers

    # Run tests in parallel with specified workers
    parallelize(workers: :number_of_processors)

    # Setup all fixtures in test/fixtures/*.yml for all tests in alphabetical order.
    self.use_transactional_tests = true

    def with_dry_run(value)
      original = Rails.configuration.dry_run
      Rails.configuration.dry_run = value
      yield
    ensure
      Rails.configuration.dry_run = original
    end
  end
end

module ActionDispatch
  class IntegrationTest
    include Devise::Test::IntegrationHelpers
  end
end

module DeviseHelpers
  def sign_in_user
    user = User.create(
      email: 'test@test.com',
      password: 'password',
      password_confirmation: 'password',
      confirmed_at: Time.now
    )
    sign_in user
    user
  end
end
