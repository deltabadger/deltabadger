ENV['RAILS_ENV'] ||= 'test'

# The suite's baseline is a self-hosted container: no platform market data.
# Tests that exercise the hosted path opt in by setting MARKET_DATA_URL themselves
# and clear it again afterwards. A developer with these exported in their shell
# (handy for pointing `rails server` at a real data-api) would otherwise flip the
# whole suite into hosted mode and fail every stock/market-data assertion.
# Same intent as WebMock.disable_net_connect! below: cut the suite off from ambient config.
#
# The encryption keys are here for the same reason and one more: with them set, the app
# encrypts under those instead of deriving from secret_key_base, so a developer who had
# exported them would run a suite configured differently from CI.
%w[
  MARKET_DATA_URL MARKET_DATA_TOKEN MARKET_DATA_PROVIDER_NAME
  ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT
  ACTIVE_RECORD_ENCRYPTION_KEYS_EXTERNAL
].each { |key| ENV.delete(key) }

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

# A tool only ever runs inside an authenticated MCP request, where the bearer token
# names both the user and the OAuth client. Unit tests have to supply both or the
# per-call gate refuses the tool — which is exactly what production does.
#
# The default grant is the user's own enabled set, matching what RecordConsent can
# actually produce. Granting the whole catalogue instead would make the intersection
# a no-op in every test that uses this, and the client half of the rule could then be
# deleted with the suite still green.
module MCPToolTestHelper
  def stub_mcp_client(user, mcp_tools: nil)
    application = Doorkeeper::Application.create!(
      name: 'Test client', redirect_uri: 'http://localhost/callback',
      confidential: false, scopes: 'mcp'
    )
    ConnectedClient.create!(
      user: user, oauth_application: application,
      mcp_tools: mcp_tools || user.enabled_mcp_tool_names
    )
    ActionMCP::Current.stubs(:user).returns(user)
    OauthClientContext.oauth_application = application
    application
  end
end

module MarketDataTestHelper
  def configure_deltabadger_market_data
    AppConfig.market_data_provider = MarketDataSettings::PROVIDER_DELTABADGER
    AppConfig.market_data_url = 'https://market-data.example.com'
    AppConfig.market_data_token = 'test_market_data_token'
  end

  def clear_market_data_configuration
    keys = [
      AppConfig::MARKET_DATA_PROVIDER,
      AppConfig::MARKET_DATA_URL,
      AppConfig::MARKET_DATA_TOKEN
    ]
    AppConfig.where(key: keys).delete_all
  end
end

module ActiveSupport
  class TestCase
    include FactoryBot::Syntax::Methods
    include ExchangeMockHelpers
    include MCPToolTestHelper
    include MarketDataTestHelper

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
