require 'test_helper'

# An ordinary integration test of the pending-email lifecycle. The confirmation
# token is read from the test user's row, never printed or sent to another host.
class SettingsEmailConfirmationTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true, wash_sale_enabled: false)
    sign_in @user
    @original_csrf = ActionController::Base.allow_forgery_protection
    ActionController::Base.allow_forgery_protection = true
    host! 'localhost:3000'
    get settings_account_path
    assert_response :success
    @csrf = Nokogiri::HTML(response.body).at_css('meta[name="csrf-token"]')['content']
  end

  teardown do
    ActionController::Base.allow_forgery_protection = @original_csrf
  end

  test 'an authorized email change remains pending until token confirmation' do
    original = @user.email
    patch settings_update_email_path,
          params: { user: { email: 'next@example.com', current_password: 'TestPassword1!' } },
          headers: { 'Accept' => 'text/vnd.turbo-stream.html', 'X-CSRF-Token' => @csrf,
                     'Origin' => 'http://localhost:3000' }
    assert_response :success
    assert_equal original, @user.reload.email
    assert_equal 'next@example.com', @user.unconfirmed_email
    assert @user.confirmation_token.present?

    # GET supplies the email token but neither the browser's CSRF token nor a
    # password. A foreign Origin does not prevent finalizing the pending change.
    get user_confirmation_path, params: { confirmation_token: @user.confirmation_token },
                                headers: { 'Origin' => 'http://foreign.example' }
    assert_response :redirect
    assert_equal 'next@example.com', @user.reload.email
    assert_nil @user.unconfirmed_email
    assert_equal true, ActionController::Base.allow_forgery_protection
  end

  test 'a reused confirmation token is refused without changing the account again' do
    begin_email_change
    token = @user.reload.confirmation_token
    get user_confirmation_path, params: { confirmation_token: token }
    assert_response :redirect
    before = @user.reload.attributes
    get user_confirmation_path, params: { confirmation_token: token }
    # Rails renders the resend form with 200 on this base; refusal is the
    # unchanged account and absence of a successful-confirmation redirect.
    assert_response :success
    assert_select 'input[name="user[email]"]'
    assert @user.reload.attributes == before, 'reused token changed the stored account'
  end

  test 'a wrong confirmation token is refused without changing the pending account' do
    begin_email_change
    before = @user.reload.attributes
    get user_confirmation_path, params: { confirmation_token: 'wrong-placeholder-token' }
    assert_response :success
    assert_select 'input[name="user[email]"]'
    assert @user.reload.attributes == before, 'wrong token changed the stored account'
  end

  test 'Rails accepts a century old pending confirmation with its default configuration' do
    begin_email_change
    assert_nil User.confirm_within
    @user.update_columns(confirmation_sent_at: 100.years.ago)
    get user_confirmation_path, params: { confirmation_token: @user.confirmation_token },
                                headers: { 'Origin' => 'http://foreign.example' }
    assert_response :redirect
    assert_equal 'next@example.com', @user.reload.email
    assert_nil @user.unconfirmed_email
  end

  test 'a newer email request replaces the token and refuses the superseded token' do
    begin_email_change
    old_token = @user.reload.confirmation_token
    patch settings_update_email_path,
          params: { user: { email: 'later@example.com', current_password: 'TestPassword1!' } },
          headers: { 'Accept' => 'text/vnd.turbo-stream.html', 'X-CSRF-Token' => @csrf,
                     'Origin' => 'http://localhost:3000' }
    assert_response :success
    assert_not_equal old_token, @user.reload.confirmation_token
    before = @user.attributes
    get user_confirmation_path, params: { confirmation_token: old_token }
    assert_response :success
    assert_select 'input[name="user[email]"]'
    assert @user.reload.attributes == before, 'superseded token changed the account'
    get user_confirmation_path, params: { confirmation_token: @user.confirmation_token }
    assert_response :redirect
    assert_equal 'later@example.com', @user.reload.email
    assert_nil @user.unconfirmed_email
  end

  test 'taken-address request clears authorization sent to the previous address' do
    assert taken_address_change_using_old_token
  end

  # Upstream 8d06dfd1 now fixes M too. Historical failing safety evidence is
  # retained in evidence/settings_email_confirmation_security_red.rb.txt.

  private

  # Freeing the address is another owner's ordinary password-authorized PATCH
  # and emailed-token confirmation, not a direct database write or deletion.
  def taken_address_change_using_old_token
    other = create(:user, email: 'taken@example.com', setup_completed: true, wash_sale_enabled: false)
    ActionMailer::Base.deliveries.clear
    begin_email_change
    first_token = @user.reload.confirmation_token
    assert(ActionMailer::Base.deliveries.any? { |mail| mail.to == ['next@example.com'] })
    patch settings_update_email_path,
          params: { user: { email: 'taken@example.com', current_password: 'TestPassword1!' } },
          headers: { 'Accept' => 'text/vnd.turbo-stream.html', 'X-CSRF-Token' => @csrf,
                     'Origin' => 'http://localhost:3000' }
    assert_response :success
    assert_equal 'taken@example.com', @user.reload.unconfirmed_email
    assert_nil @user.confirmation_token
    assert_nil @user.confirmation_sent_at
    refute(ActionMailer::Base.deliveries.any? { |mail| mail.to == ['taken@example.com'] })

    sign_out @user
    sign_in other
    get settings_account_path
    assert_response :success
    csrf = Nokogiri::HTML(response.body).at_css('meta[name="csrf-token"]')['content']
    patch settings_update_email_path,
          params: { user: { email: 'moved@example.com', current_password: 'TestPassword1!' } },
          headers: { 'Accept' => 'text/vnd.turbo-stream.html', 'X-CSRF-Token' => csrf,
                     'Origin' => 'http://localhost:3000' }
    assert_response :success
    assert_equal 'moved@example.com', other.reload.unconfirmed_email
    assert(ActionMailer::Base.deliveries.any? { |mail| mail.to == ['moved@example.com'] })
    get user_confirmation_path, params: { confirmation_token: other.confirmation_token }
    assert_response :redirect
    assert_equal 'moved@example.com', other.reload.email
    assert_nil other.unconfirmed_email

    sign_out other
    original = @user.reload.email
    get user_confirmation_path, params: { confirmation_token: first_token },
                                headers: { 'Origin' => 'http://foreign.example' }
    assert_response :success
    assert_equal original, @user.reload.email
    assert_equal 'taken@example.com', @user.unconfirmed_email
    refute ActionMailer::Base.deliveries.any? { |mail| mail.to == ['taken@example.com'] },
           'no confirmation message was ever sent to the later address'
    @user.email == original
  end

  def begin_email_change
    patch settings_update_email_path,
          params: { user: { email: 'next@example.com', current_password: 'TestPassword1!' } },
          headers: { 'Accept' => 'text/vnd.turbo-stream.html', 'X-CSRF-Token' => @csrf,
                     'Origin' => 'http://localhost:3000' }
    assert_response :success
    assert_equal 'next@example.com', @user.reload.unconfirmed_email
    assert @user.confirmation_token.present?
  end
end
