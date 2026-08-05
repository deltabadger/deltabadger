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
end
