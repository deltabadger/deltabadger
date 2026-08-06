require 'test_helper'

class UserTest < ActiveSupport::TestCase
  test 'a junk locale is rejected and not persisted' do
    user = create(:user, locale: 'en')

    refute user.update(locale: 'zz')
    assert_includes user.errors[:locale], 'is not included in the list'
    assert_equal 'en', user.reload.locale
  end

  test 'a real locale is accepted' do
    user = create(:user)

    assert user.update(locale: 'pl')
    assert_equal 'pl', user.reload.locale
  end

  test 'no locale preference is valid' do
    user = build(:user, locale: nil)
    assert_predicate user, :valid?

    user.locale = ''
    assert_predicate user, :valid?
  end

  test 'generates a two-factor secret when the seed was cleared' do
    user = create(:user)
    User.where(id: user.id).update_all(otp_secret_key: nil)

    user.reload.ensure_two_factor_secret!

    assert user.reload.otp_secret_key.present?
  end

  # Must NOT rotate a working secret - a user with two-factor already set up would be
  # silently locked out of their authenticator app.
  test 'leaves an existing two-factor secret alone' do
    user = create(:user, otp_module: :enabled)
    original = user.otp_secret_key
    assert original.present?, 'expected the create hook to seed one'

    user.ensure_two_factor_secret!

    assert_equal original, user.reload.otp_secret_key
  end
end
