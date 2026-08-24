# frozen_string_literal: true

require 'test_helper'

# What the layouts tell a browser to fetch. Both facts below were visible only in a console:
# a desktop-only script sitting in every page's <head>, and preload headers for assets the
# page already had.
class LayoutAssetsTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  setup { @admin = create(:user, admin: true, setup_completed: true) }

  # The desktop guard is a Tauri initialization script now (src-tauri/src/lib.rs), so nothing
  # tauri-shaped is fetched over the web. Matching the asset name rather than a tag keeps this
  # honest if the file returns under a different include.
  test 'no layout asks a browser for a tauri asset' do
    signed_out_then_in do |path|
      assert_no_match(/tauri/i, asset_urls.join(' '), "#{path} still fetches a tauri asset")
    end
  end

  # config.action_view.preload_links_header is off: the header only duplicated the tags, and
  # Turbo made every visit start preloads nothing consumed.
  test 'no layout emits preload link headers' do
    signed_out_then_in do |path|
      assert_nil response.headers['Link'], "#{path} still advertises preloads"
    end
  end

  private

  # /en/login is requested before signing in: Devise redirects a signed-in user away from it,
  # and a redirect body would satisfy either assertion while inspecting nothing.
  def signed_out_then_in
    get '/en/login'
    assert_response :success, '/en/login did not render, so it proved nothing'
    yield '/en/login'

    sign_in @admin
    ['/en/bots', '/en/tracker'].each do |path|
      get path
      assert_response :success, "#{path} did not render, so it proved nothing"
      yield path
    end
  end

  def asset_urls
    css_select('script[src], link[href]').map { |node| node['src'] || node['href'] }
  end
end
