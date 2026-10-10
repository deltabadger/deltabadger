require 'test_helper'
require 'stringio'

# S1's log requirement uses only placeholder credentials and WebMock responses.
# The real controller, model, exchange validator and HTTP client run unchanged.
class SettingsCredentialLoggingTest < ActionDispatch::IntegrationTest
  PLACEHOLDER = 'placeholder-value-123'.freeze
  TURBO_STREAM = { 'Accept' => 'text/vnd.turbo-stream.html, text/html' }.freeze

  setup do
    @user = create(:user, admin: true, setup_completed: true, wash_sale_enabled: false)
    @alpaca = create(:alpaca_exchange)
    @key = ApiKey.create!(user: @user, exchange: @alpaca, key_type: :trading,
                          status: :correct, key: 'previous-key', secret: 'previous-secret', passphrase: 'paper')
    sign_in @user
    @original_csrf = ActionController::Base.allow_forgery_protection
    ActionController::Base.allow_forgery_protection = true
    host! 'localhost:3000'
    get settings_connect_path
    assert_response :success
    csrf = Nokogiri::HTML(response.body).at_css('meta[name="csrf-token"]')
    assert csrf, 'the settings page must supply a CSRF token'
    @headers = TURBO_STREAM.merge('X-CSRF-Token' => csrf['content'], 'Origin' => 'http://localhost:3000')
    @log = StringIO.new
    @capture_logger = ActiveSupport::Logger.new(@log)
    @capture_logger.level = Logger::DEBUG
    Rails.logger.broadcast_to(@capture_logger)
  end

  teardown do
    Rails.logger.stop_broadcasting_to(@capture_logger) if @capture_logger
    ActionController::Base.allow_forgery_protection = @original_csrf
  end

  test 'saving the stocks settings form does not log the placeholder' do
    request = account_response(200, { status: 'ACTIVE' })
    with_dry_run(false) do
      patch settings_update_stocks_path,
            params: { alpaca_api_key: 'placeholder-key', alpaca_api_secret: PLACEHOLDER, alpaca_mode: 'paper' },
            headers: @headers
    end
    assert_response :success
    assert_requested request, times: 1
    assert_equal PLACEHOLDER, AppConfig.get('alpaca_api_secret')
    assert_includes @log.string, 'SettingsController#update_stocks'
    refute_includes @log.string, PLACEHOLDER
    refute_includes response.body, PLACEHOLDER
  end

  test 'saving an exchange credential does not log the placeholder' do
    request = account_response(200, { status: 'ACTIVE' })
    submit_key
    assert_response :success
    assert_requested request, times: 1
    assert_equal PLACEHOLDER, @key.reload.secret
    refute_includes @log.string, PLACEHOLDER
    refute_includes response.body, PLACEHOLDER
  end

  test 'a failed credential save does not log the placeholder' do
    assert_failed_save_does_not_log_placeholder('validation service unavailable')
  end

  test 'a failed credential save with a service message containing the placeholder does not log it' do
    assert_failed_save_does_not_log_placeholder("validation service unavailable for #{PLACEHOLDER}")
  end

  private

  def account_response(status, body)
    stub_request(:get, 'https://paper-api.alpaca.markets/v2/account')
      .with(headers: { 'APCA-API-KEY-ID' => 'placeholder-key', 'APCA-API-SECRET-KEY' => PLACEHOLDER })
      .to_return(status: status, body: JSON.generate(body), headers: { 'Content-Type' => 'application/json' })
  end

  def submit_key
    with_dry_run(false) do
      post tracker_add_api_key_path,
           params: { exchange_id: @alpaca.id, key_type: 'trading',
                     api_key: { key: 'placeholder-key', secret: PLACEHOLDER, passphrase: 'paper' } },
           headers: @headers
    end
  end

  def assert_failed_save_does_not_log_placeholder(message)
    @key.update!(key: 'placeholder-key', secret: PLACEHOLDER)
    before = @key.reload.attributes
    encrypted_secret = ApiKey.connection.select_value("SELECT secret FROM api_keys WHERE id = #{Integer(@key.id)}")
    assert JSON.parse(encrypted_secret).fetch('h').key?('at'), 'the existing secret must be encrypted at rest'
    refute_includes encrypted_secret, PLACEHOLDER
    @log.truncate(0)
    @log.rewind
    request = account_response(503, { message: message })
    submit_key
    assert_response :unprocessable_entity
    assert_requested request, times: 1
    assert_equal before, @key.reload.attributes
    assert_includes response.body, I18n.t('errors.api_key_permission_validation_failed')
    assert_includes @log.string, '[Alpaca] API key validation failed:'
    refute_includes response.body, PLACEHOLDER
    refute_includes @log.string, PLACEHOLDER
  end
end
