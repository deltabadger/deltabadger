# frozen_string_literal: true

require 'test_helper'

class Api::V1::TaxReportsControllerTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user)
    @oauth_app = Doorkeeper::Application.create!(
      name: 'Test', redirect_uri: 'http://localhost/callback', confidential: false, scopes: 'api'
    )
    ConnectedClient.create!(user: @user, oauth_application: @oauth_app, rest_tools: AppConfig::REST_TOOL_DEFAULTS.keys)
    MarketData.stubs(:configured?).returns(true)
    @written = []
  end

  teardown do
    @written.each { |path| FileUtils.rm_f(path) }
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.destroy_all
  end

  test 'GET /tax/jurisdictions lists countries' do
    @user.set_rest_tool_enabled('list_tax_jurisdictions', true)
    get '/api/v1/tax/jurisdictions', headers: bearer(api_token)
    assert_response :ok
    codes = JSON.parse(response.body)['data']['jurisdictions'].map { |row| row['code'] }
    assert_includes codes, 'DE'
  end

  test 'POST /tax/reports returns 202 and enqueues' do
    @user.set_rest_tool_enabled('generate_tax_report', true)
    Tax::GenerateReportJob.expects(:perform_later).with(@user.id, 'DE', 2043, false)
                          .returns(stub(successfully_enqueued?: true, provider_job_id: nil))
    post '/api/v1/tax/reports', params: { country: 'DE', year: 2043 }, headers: bearer(api_token)
    assert_response :accepted
    assert_equal false, JSON.parse(response.body)['data']['ready']
  end

  # No route constraints: a malformed path reaches the service and gets the envelope, not a
  # routing 404.
  test 'malformed path parameters are documented 422s' do
    @user.set_rest_tool_enabled('get_tax_report_status', true)
    @user.set_rest_tool_enabled('download_tax_report', true)
    get '/api/v1/tax/reports/XX/2044', headers: bearer(api_token)
    assert_response :unprocessable_entity
    assert_equal 'unknown_country', JSON.parse(response.body)['error']['code']
    get '/api/v1/tax/reports/DE/20x5/download', headers: bearer(api_token)
    assert_response :unprocessable_entity
    assert_equal 'invalid_year', JSON.parse(response.body)['error']['code']
  end

  test 'GET /tax/reports/:country/:year reports readiness' do
    @user.set_rest_tool_enabled('get_tax_report_status', true)
    get '/api/v1/tax/reports/DE/2045', headers: bearer(api_token)
    assert_response :ok
    body = JSON.parse(response.body)['data']
    assert_equal false, body['ready']
    assert_equal 'none', body['state']
  end

  test 'GET …/download serves the CSV and leaves the file in place' do
    @user.set_rest_tool_enabled('download_tax_report', true)
    path = write_report('DE', 2046, "a,b\n1,2\n")
    get '/api/v1/tax/reports/DE/2046/download', headers: bearer(api_token)
    assert_response :ok
    assert_equal 'text/csv', response.media_type
    assert_equal "a,b\n1,2\n", response.body
    assert File.exist?(path)
  end

  test 'download of a missing report is a JSON 404' do
    @user.set_rest_tool_enabled('download_tax_report', true)
    get '/api/v1/tax/reports/DE/2047/download', headers: bearer(api_token)
    assert_response :not_found
    assert_equal 'report_not_found', JSON.parse(response.body)['error']['code']
  end

  test 'each action is gated by its own tool' do
    get '/api/v1/tax/jurisdictions', headers: bearer(api_token)
    assert_response :forbidden
    assert_equal 'tool_disabled', JSON.parse(response.body)['error']['code']
  end

  private

  # tmp/tax_reports is shared by every parallel worker and user ids repeat across their
  # databases, so each test that touches a file owns a year of its own.
  def write_report(country, year, contents)
    path = Tax::GenerateReportJob.report_path(@user.id, country, year)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, contents)
    @written << path
    path
  end

  def bearer(token) = { 'Authorization' => "Bearer #{token.token}" }

  def api_token
    Doorkeeper::AccessToken.create!(
      application: @oauth_app, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'api', expires_in: 3600
    )
  end
end
