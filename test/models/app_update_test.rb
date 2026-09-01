require 'test_helper'

# The update notice is cosmetic, and a cosmetic feature may never break the app or slow a page
# down: nothing here reaches the network on render, and no failure escapes.
class AppUpdateTest < ActiveSupport::TestCase
  RELEASES_URL = 'https://api.github.com/repos/deltabadger/deltabadger/releases/latest'.freeze

  setup do
    # The suite runs :null_store, which would make every cache assertion below vacuously pass.
    @original_cache = Rails.cache
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Rails.application.config.stubs(:version).returns('2.46.3')
  end

  teardown do
    Rails.cache = @original_cache
  end

  def stub_release(tag, status: 200, body: nil)
    payload = body || { tag_name: tag, html_url: "https://github.com/deltabadger/deltabadger/releases/tag/#{tag}" }
    stub_request(:get, RELEASES_URL).to_return(
      status: status,
      body: payload.is_a?(String) ? payload : payload.to_json,
      headers: { 'Content-Type' => 'application/json' }
    )
  end

  test 'a newer release is available, with its version and URL' do
    stub_release('v2.47.0')
    AppUpdate.refresh!

    assert AppUpdate.available?
    assert_equal '2.47.0', AppUpdate.latest_version
    assert_equal 'https://github.com/deltabadger/deltabadger/releases/tag/v2.47.0', AppUpdate.latest_url
  end

  # The response is remote input that ends up in a link on a settings page. The link is built from
  # the tag, which the release pattern has already validated.
  test 'the release link is built from the tag, not taken from the payload' do
    stub_release(nil, body: { tag_name: 'v2.47.0', html_url: 'javascript:alert(1)' })
    AppUpdate.refresh!

    assert_equal 'https://github.com/deltabadger/deltabadger/releases/tag/v2.47.0', AppUpdate.latest_url
  end

  test 'the running version is not an update' do
    stub_release('v2.46.3')
    AppUpdate.refresh!

    refute AppUpdate.available?
  end

  test 'an older release is not an update' do
    stub_release('v2.46.2')
    AppUpdate.refresh!

    refute AppUpdate.available?
  end

  # String ordering puts '2.9.0' after '2.10.0', which would hide every update for ten releases
  # after each minor bump.
  test 'versions compare numerically, not as strings' do
    Rails.application.config.stubs(:version).returns('2.9.0')
    stub_release('v2.10.0')
    AppUpdate.refresh!

    assert AppUpdate.available?
  end

  test 'tags that are not plain releases are ignored' do
    ['nightly', 'v2.47.0-rc1', 'latest', ''].each do |tag|
      Rails.cache.clear
      stub_release(tag)
      AppUpdate.refresh!

      assert_nil AppUpdate.latest_version, "#{tag.inspect} must not be read as a release"
      refute AppUpdate.available?
    end
  end

  test 'reading never issues a request' do
    assert_nil AppUpdate.latest_version
    refute AppUpdate.available?
    assert_not_requested :get, RELEASES_URL
  end

  test 'a refresh serves later reads from the cache' do
    stub_release('v2.47.0')
    AppUpdate.refresh!
    AppUpdate.latest_version
    AppUpdate.available?

    assert_requested :get, RELEASES_URL, times: 1
  end

  test 'a hosted install never asks' do
    Installation.stubs(:platform).returns(:hosted)

    refute AppUpdate.enabled?
    assert_nil AppUpdate.refresh!
    assert_not_requested :get, RELEASES_URL
  end

  test 'a desktop install never asks — its own updater is the authority' do
    Installation.stubs(:platform).returns(:desktop)

    refute AppUpdate.enabled?
    assert_nil AppUpdate.refresh!
    assert_not_requested :get, RELEASES_URL
  end

  # The only outbound call an otherwise offline install makes, so it has an off switch — and the
  # switch has to understand how people actually write "off". ActiveModel::Type::Boolean reads
  # 'no' as true, which is why this goes through the application's own resolver.
  test 'the opt-out understands the usual spellings of off' do
    %w[false FALSE 0 f n no NO off OFF].each do |value|
      with_update_check(value) do
        refute AppUpdate.enabled?, "DELTABADGER_UPDATE_CHECK=#{value} must disable the check"
        assert_nil AppUpdate.refresh!
      end
    end
    assert_not_requested :get, RELEASES_URL
  end

  test 'an unset or unrecognised opt-out leaves the check on' do
    with_update_check(nil) { assert AppUpdate.enabled? }
    with_update_check('perhaps') { assert AppUpdate.enabled? }
    with_update_check('yes') { assert AppUpdate.enabled? }
  end

  test 'a network failure is not an error' do
    stub_request(:get, RELEASES_URL).to_timeout

    assert_nil AppUpdate.refresh!
    refute AppUpdate.available?
  end

  test 'a server error is not an error' do
    stub_release('v2.47.0', status: 503)

    assert_nil AppUpdate.refresh!
    refute AppUpdate.available?
  end

  test 'a malformed body is not an error' do
    stub_release(nil, body: 'not json at all')

    assert_nil AppUpdate.refresh!
    refute AppUpdate.available?
  end

  # An outage must not blank a notice the user has already been shown.
  test 'a failed refresh leaves the last good answer standing' do
    stub_release('v2.47.0')
    AppUpdate.refresh!
    remove_request_stub(WebMock::StubRegistry.instance.request_stubs.first)
    stub_request(:get, RELEASES_URL).to_timeout

    AppUpdate.refresh!

    assert_equal '2.47.0', AppUpdate.latest_version
    assert AppUpdate.available?
  end

  private

  def with_update_check(value)
    original = ENV['DELTABADGER_UPDATE_CHECK']
    if value.nil?
      ENV.delete('DELTABADGER_UPDATE_CHECK')
    else
      ENV['DELTABADGER_UPDATE_CHECK'] = value
    end
    yield
  ensure
    original.nil? ? ENV.delete('DELTABADGER_UPDATE_CHECK') : ENV['DELTABADGER_UPDATE_CHECK'] = original
  end
end
