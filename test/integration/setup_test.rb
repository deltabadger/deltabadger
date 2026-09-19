require 'test_helper'

class SetupTest < ActionDispatch::IntegrationTest
  # == When no admin exists ==

  test 'shows the setup form when no admin exists' do
    get new_setup_path
    assert_response :ok
  end

  test 'renders the optional platform claim frame outside the non-Turbo setup form' do
    get new_setup_path

    assert_response :ok
    assert_select 'form[data-turbo="false"] turbo-frame#setup_platform_connection', count: 0
    assert_select 'turbo-frame#setup_platform_connection', count: 1 do
      assert_select "form[action='#{setup_platform_connection_path}']"
      assert_select 'input[name=claim_code]'
    end
  end

  test 'connects a pasted claim and streams identity into the untouched setup form' do
    Platform::RedeemClaim.expects(:call).with(code: 'dbc_manual').returns(
      Result::Success.new(email: 'owner@example.com', name: 'Owner')
    )

    post setup_platform_connection_path, params: { claim_code: 'dbc_manual' }, as: :turbo_stream

    assert_response :success
    assert_select 'turbo-stream[action=replace][target=setup_platform_connection]'
    assert_select 'turbo-stream[action=replace][target=setup_identity_fields]'
    assert_includes response.body, 'owner@example.com'
    assert_includes response.body, 'Owner'
  end

  test 'keeps a pasted claim identity across a setup reload' do
    Platform::RedeemClaim.expects(:call).with(code: 'dbc_manual').returns(
      Result::Success.new(email: 'owner@example.com', name: 'Owner')
    )

    post setup_platform_connection_path, params: { claim_code: 'dbc_manual' }, as: :turbo_stream
    AppConfig.set('platform_connected_at', Time.current.iso8601)
    get new_setup_path(locale: :de)

    assert_response :success
    assert_select 'input#user_name[value=?]', 'Owner'
    assert_select 'input#user_email[value=?]', 'owner@example.com'
  end

  test 'shows a claim failure inline without replacing the setup identity fields' do
    Platform::RedeemClaim.expects(:call).with(code: 'expired').returns(
      Result::Failure.new('That claim code is invalid or has expired.')
    )

    post setup_platform_connection_path, params: { claim_code: 'expired' }, as: :turbo_stream

    assert_response :unprocessable_entity
    assert_select 'turbo-stream[action=replace][target=setup_platform_connection]'
    assert_select 'turbo-stream[target=setup_identity_fields]', count: 0
    assert_includes response.body, 'That claim code is invalid or has expired.'
  end

  test 'auto-redeems CLAIM_TOKEN and prefills the account form' do
    with_env('CLAIM_TOKEN', 'dbc_from_docker') do
      Platform::RedeemClaim.expects(:call).with(code: 'dbc_from_docker').returns(
        Result::Success.new(email: 'docker@example.com', name: 'Docker Owner')
      )

      get new_setup_path

      assert_response :ok
      assert_select 'input#user_name[value=?]', 'Docker Owner'
      assert_select 'input#user_email[value=?]', 'docker@example.com'
      assert_select 'turbo-frame#setup_platform_connection .form__info--notice'
    end
  end

  test 'does not redeem CLAIM_TOKEN after the platform is already connected' do
    AppConfig.set('platform_connected_at', Time.current.iso8601)

    with_env('CLAIM_TOKEN', 'dbc_already_used') do
      Platform::RedeemClaim.expects(:call).never
      get new_setup_path
    end

    assert_response :ok
    assert_select 'turbo-frame#setup_platform_connection .form__info--notice'
  end

  test 'connected setup copy distinguishes a claim without proxies' do
    AppConfig.set('platform_connected_at', Time.current.iso8601)

    get new_setup_path

    assert_response :ok
    assert_includes response.body, I18n.t('setup.platform.connected_without_proxies')
    assert_not_includes response.body, I18n.t('setup.platform.connected_with_proxies')
  end

  test 'a failed CLAIM_TOKEN is dismissable and does not block independent setup' do
    with_env('CLAIM_TOKEN', 'expired') do
      Platform::RedeemClaim.expects(:call).with(code: 'expired').returns(
        Result::Failure.new('That claim code is invalid or has expired.')
      )

      get new_setup_path
    end

    assert_response :ok
    assert_select '.salert--danger[data-controller=removals] button[data-action="removals#remove"]'
    assert_select 'form[data-turbo=false] input#user_password'
  end

  test 'creates admin account with valid credentials' do
    assert_difference 'User.count', 1 do
      post setup_path, params: {
        user: { name: 'Admin', email: 'admin@example.com', password: 'SecurePass1!' }
      }
    end

    user = User.last
    assert_equal true, user.admin
    assert user.confirmed_at.present?
    assert_redirected_to bots_path
  end

  test 'creates an independent admin without involving the platform' do
    Platform::RedeemClaim.expects(:call).never

    assert_difference 'User.count', 1 do
      post setup_path, params: {
        user: { name: 'Independent', email: 'independent@example.com', password: 'SecurePass1!' }
      }
    end

    assert_redirected_to bots_path
    assert_nil AppConfig.get('platform_connected_at')
    assert_nil AppConfig.market_data_provider
  end

  test 'signs in the new admin after creation' do
    post setup_path, params: {
      user: { name: 'Admin', email: 'admin@example.com', password: 'SecurePass1!' }
    }
    follow_redirect!

    assert controller.current_user.present?
    assert_equal true, controller.current_user.admin
  end

  test 'rejects invalid credentials during setup' do
    post setup_path, params: {
      user: { name: '', email: 'invalid', password: 'weak' }
    }

    assert_response :unprocessable_content
    assert_equal 0, User.count
  end

  test 'failed admin signup keeps a connected install connected in the rendered form' do
    AppConfig.set('platform_connected_at', Time.current.iso8601)

    post setup_path, params: {
      user: { name: '', email: 'invalid', password: 'weak' }
    }

    assert_response :unprocessable_content
    assert_select 'turbo-frame#setup_platform_connection .form__info--notice'
    assert_select "form[action='#{setup_platform_connection_path}']", count: 0
  end

  test 'rejects missing password during setup' do
    post setup_path, params: {
      user: { name: 'Admin', email: 'admin@example.com', password: '' }
    }

    assert_response :unprocessable_content
    assert_equal 0, User.count
  end

  # == When SETUP_TOKEN is set ==

  test 'locks the setup form when the token is missing' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path

      assert_response :forbidden
      assert_select 'h1', I18n.t('setup.locked.title')
      assert_select 'input#user_email', false
      assert_select 'a.dropdown__item[href=?]', '/setup?locale=de'
    end
  end

  test 'the locked page follows the locale parameter' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path(locale: 'de')

      assert_response :forbidden
      assert_select 'html[lang=de]'
    end
  end

  test 'locks the setup form when the token is wrong' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path(token: 'wrong-token')

      assert_response :forbidden
    end
  end

  test 'a blank SETUP_TOKEN locks setup and matches nothing, not even a blank token' do
    with_env('SETUP_TOKEN', '') do
      get new_setup_path
      assert_response :forbidden

      get new_setup_path(token: '')
      assert_response :forbidden

      assert_no_difference 'User.count' do
        post setup_path, params: { token: '', user: { name: 'Admin', email: 'admin@example.com', password: 'SecurePass1!' } }
      end
      assert_response :forbidden
    end
  end

  test 'a malformed token parameter is rejected rather than raising' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get '/setup?token[]=expected-token'
      assert_response :forbidden

      get '/setup?token[a]=expected-token'
      assert_response :forbidden
    end
  end

  test 'the locked page answers non-HTML requests with 403 as well' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get '/setup.json'

      assert_response :forbidden
    end
  end

  test 'does not create an admin without the token' do
    with_env('SETUP_TOKEN', 'expected-token') do
      assert_no_difference 'User.count' do
        post setup_path, params: { user: { name: 'Admin', email: 'admin@example.com', password: 'SecurePass1!' } }
      end

      assert_response :forbidden
    end
  end

  test 'does not redeem a pasted claim code without the token' do
    with_env('SETUP_TOKEN', 'expected-token') do
      Platform::RedeemClaim.expects(:call).never

      post setup_platform_connection_path, params: { claim_code: 'dbc_manual' }, as: :turbo_stream

      assert_response :forbidden
    end
  end

  test 'does not auto-redeem CLAIM_TOKEN while setup is locked' do
    with_env('SETUP_TOKEN', 'expected-token') do
      with_env('CLAIM_TOKEN', 'dbc_from_docker') do
        Platform::RedeemClaim.expects(:call).never

        get new_setup_path

        assert_response :forbidden
      end
    end
  end

  test 'the token unlocks setup, leaves the URL, and lets the admin be created' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path(token: 'expected-token')
      assert_redirected_to new_setup_path

      follow_redirect!
      assert_response :ok
      assert_select 'form[action^="/setup"]'

      assert_difference 'User.count', 1 do
        post setup_path, params: { user: { name: 'Admin', email: 'admin@example.com', password: 'SecurePass1!' } }
      end
      assert_redirected_to bots_path
      assert_predicate User.last, :admin?
      assert_nil session[:setup_token]
    end
  end

  test 'an unlocked session can switch language and redeem a claim code' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path(token: 'expected-token')

      get new_setup_path(locale: 'de')
      assert_response :ok

      Platform::RedeemClaim.expects(:call).with(code: 'dbc_manual').returns(
        Result::Success.new(email: 'owner@example.com', name: 'Owner')
      )
      post setup_platform_connection_path, params: { claim_code: 'dbc_manual' }, as: :turbo_stream
      assert_response :success
    end
  end

  test 'a session unlocked with an old token is locked again when SETUP_TOKEN changes' do
    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path(token: 'expected-token')
    end

    with_env('SETUP_TOKEN', 'rotated-token') do
      get new_setup_path

      assert_response :forbidden
    end
  end

  test 'an existing admin still redirects to root when SETUP_TOKEN is set' do
    create(:user, admin: true)

    with_env('SETUP_TOKEN', 'expected-token') do
      get new_setup_path
      assert_redirected_to root_path

      get new_setup_path(token: 'expected-token')
      assert_redirected_to root_path
    end
  end

  test 'locked page copy exists in every locale' do
    I18n.available_locales.each do |locale|
      %w[title text].each do |key|
        assert I18n.exists?("setup.locked.#{key}", locale, fallback: false), "missing setup.locked.#{key} in #{locale}"
      end
    end
  end

  # == When admin already exists ==

  test 'redirects away from setup form when admin exists' do
    create(:user, admin: true)

    get new_setup_path
    assert_redirected_to root_path
  end

  test 'prevents creating another admin when one exists' do
    create(:user, admin: true)

    assert_no_difference 'User.count' do
      post setup_path, params: {
        user: { name: 'Admin2', email: 'admin2@example.com', password: 'SecurePass1!' }
      }
    end

    assert_redirected_to root_path
  end

  private

  def with_env(key, value)
    original = ENV[key]
    ENV[key] = value
    yield
  ensure
    original.nil? ? ENV.delete(key) : ENV[key] = original
  end
end
