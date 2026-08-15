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

# Tax::EcbFxRates.ensure_loaded! runs from every Tax::PriceService.new, so any test that builds a
# tax report reaches for the ECB feed. Its `rescue StandardError` cannot absorb that here —
# WebMock::NetConnectNotAllowedError descends from Exception, not StandardError — so the suite is
# shielded with a stub instead of a wider rescue in production code. The body is the SDMX header
# and no data rows: rates import as a no-op, and tests that need rates seed fx_rates or re-stub.
ActiveSupport::TestCase.setup do
  headers_only = "CURRENCY,TIME_PERIOD,OBS_VALUE\n"
  WebMock.stub_request(:get, %r{https://data-api\.ecb\.europa\.eu}).to_return(status: 200, body: headers_only)
  WebMock.stub_request(:get, %r{https://api\.statistiken\.bundesbank\.de}).to_return(status: 200, body: headers_only)
end

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

module ActiveSupport
  class TestCase
    include FactoryBot::Syntax::Methods
    include ExchangeMockHelpers
    include MCPToolTestHelper

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
