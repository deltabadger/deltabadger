require 'test_helper'

class SetupTest < ActionDispatch::IntegrationTest
  # == When no admin exists ==

  test 'shows the setup form when no admin exists' do
    get new_setup_path
    assert_response :ok
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

  test 'rejects missing password during setup' do
    post setup_path, params: {
      user: { name: 'Admin', email: 'admin@example.com', password: '' }
    }

    assert_response :unprocessable_content
    assert_equal 0, User.count
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
end
