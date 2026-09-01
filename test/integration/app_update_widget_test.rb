require 'test_helper'

# One surface on every platform, and each platform's own path to a new version. What must never
# happen is showing someone a command that cannot update the way they installed Deltabadger.
class AppUpdateWidgetTest < ActionDispatch::IntegrationTest
  include Devise::Test::IntegrationHelpers

  COMPOSE_COMMAND = 'docker compose pull && docker compose up -d'.freeze
  RELEASE_URL = 'https://github.com/deltabadger/deltabadger/releases/tag/v2.47.0'.freeze

  setup do
    @admin = create(:user, admin: true, setup_completed: true)
    sign_in @admin
  end

  def update_available(platform)
    Installation.stubs(:platform).returns(platform)
    AppUpdate.stubs(:available?).returns(true)
    AppUpdate.stubs(:latest_version).returns('2.47.0')
    AppUpdate.stubs(:latest_url).returns(RELEASE_URL)
    get settings_account_path
    assert_response :success
  end

  test 'a Compose install gets the command it can copy' do
    update_available(:docker)

    assert_select '#app_update'
    assert_select '#app_update [data-clipboard-text-value=?]', COMPOSE_COMMAND
    assert_select '#app_update a[href=?]', RELEASE_URL
  end

  test 'an Umbrel install is sent to its App Store, not to a shell' do
    update_available(:umbrel)

    assert_select '#app_update'
    assert_select '#app_update [data-clipboard-text-value]', false
    assert_select '#app_update', /Umbrel/
  end

  # Nothing here knows how an unmarked install was started, so it may not print a command that
  # assumes one. The manual covers both docker run and Compose.
  test 'an unmarked install is sent to the manual, not to a shell' do
    update_available(:unknown)

    assert_select '#app_update'
    assert_select '#app_update [data-clipboard-text-value]', false
    assert_select '#app_update a[href*=?]', 'manual/07-updating.md'
  end

  # The desktop build can install an update itself, so it is the one platform that gets a button
  # instead of something to read.
  test 'a desktop install gets a button, not instructions' do
    Installation.stubs(:platform).returns(:desktop)
    get settings_account_path

    assert_select '#app_update'
    assert_select '#app_update button[data-desktop-update]'
    assert_select '#app_update [data-desktop-update-status]'
    assert_select '#app_update [data-clipboard-text-value]', false
    assert_select '#app_update a[href*=?]', 'releases', false
  end

  test 'no other platform offers the desktop button' do
    AppUpdate.stubs(:available?).returns(true)
    AppUpdate.stubs(:latest_version).returns('2.47.0')
    AppUpdate.stubs(:latest_url).returns(RELEASE_URL)

    %i[docker umbrel unknown hosted].each do |platform|
      Installation.stubs(:platform).returns(platform)
      get settings_account_path

      assert_select '#app_update [data-desktop-update]', false, "#{platform} must not offer the desktop button"
    end
  end

  test 'a hosted install is told nothing is needed' do
    Installation.stubs(:platform).returns(:hosted)
    get settings_account_path

    assert_select '#app_update'
    assert_select '#app_update [data-clipboard-text-value]', false
    assert_select '#app_update a[href*=?]', 'releases', false
  end

  test 'an install that is current gets no instructions' do
    Installation.stubs(:platform).returns(:docker)
    AppUpdate.stubs(:available?).returns(false)
    AppUpdate.stubs(:latest_version).returns('2.46.3')
    get settings_account_path

    assert_select '#app_update'
    assert_select '#app_update [data-clipboard-text-value]', false
  end

  # A miss is "not known yet", not "current" — a fresh install, a check switched off, or a day of
  # failed checks all read the same from here, and none of them is evidence of being up to date.
  test 'nothing is claimed before an answer exists' do
    Installation.stubs(:platform).returns(:docker)
    AppUpdate.stubs(:available?).returns(false)
    AppUpdate.stubs(:latest_version).returns(nil)
    get settings_account_path

    assert_response :success
    assert_select '#app_update', false
  end

  test 'a non-admin sees no update section' do
    sign_out @admin
    sign_in create(:user, admin: false, setup_completed: true)
    Installation.stubs(:platform).returns(:docker)
    AppUpdate.stubs(:available?).returns(true)
    get settings_account_path

    assert_response :success
    assert_select '#app_update', false
  end

  test 'rendering the page never reaches the network' do
    Installation.stubs(:platform).returns(:docker)
    get settings_account_path

    assert_response :success
    assert_not_requested :get, 'https://api.github.com/repos/deltabadger/deltabadger/releases/latest'
  end
end
