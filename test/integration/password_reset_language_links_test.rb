require 'test_helper'

# The reset link's token is a credential: the page's language links must not repeat it, while the
# form that spends it still carries it.
class PasswordResetLanguageLinksTest < ActionDispatch::IntegrationTest
  test 'the reset page keeps its token in the form and out of every language link' do
    create(:user, admin: true, setup_completed: true)
    user = create(:user, password: 'Old!Password1', setup_completed: true)
    raw_token = user.send_reset_password_instructions

    get '/password/edit', params: { reset_password_token: raw_token, keep: 'yes' }

    assert_response :success
    links = css_select('a.dropdown__item').map { |a| a['href'] }
    assert_operator links.size, :>, 1
    links.each do |href|
      refute_includes href, raw_token
      assert_includes href, 'keep=yes'
    end
    assert_equal raw_token, css_select('input[name="user[reset_password_token]"]').first['value']
  end
end
