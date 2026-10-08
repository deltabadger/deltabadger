# frozen_string_literal: true

require 'test_helper'
require 'bcrypt'

# How long POST /login takes is set by its bcrypt computations, so they must not depend on
# the account: one on every path. A second one for a two-factor account's wrong password
# told an outsider which accounts have two-factor on. Counted, not timed — a wall clock
# would be a flaky way to measure the same thing.
#
# Every bcrypt computation, verifying or hashing, goes through BCrypt::Engine.hash_secret.
module BcryptComputationCounter
  class << self
    attr_accessor :count
  end

  def hash_secret(...)
    BcryptComputationCounter.count += 1 if BcryptComputationCounter.count
    super
  end
end
BCrypt::Engine.singleton_class.prepend(BcryptComputationCounter)

class Users::SessionsBcryptCountTest < ActionDispatch::IntegrationTest
  PASSWORD = 'Sup3rSecret!pass'

  setup do
    create(:user, admin: true, setup_completed: true)
    @plain = create(:user, password: PASSWORD, setup_completed: true)
    @two_factor = create(:user, password: PASSWORD, setup_completed: true)
    @two_factor.update!(otp_secret_key: ROTP::Base32.random, otp_module: :enabled)
  end

  {
    'an unknown email' => [:unknown, 'wrong-password'],
    'an unknown email with a blank password' => [:unknown, ''],
    'a wrong password' => [:plain, 'wrong-password'],
    'the right password' => [:plain, PASSWORD],
    'a blank password' => [:plain, ''],
    'a two-factor account and a wrong password' => [:two_factor, 'wrong-password'],
    'a two-factor account and the right password' => [:two_factor, PASSWORD],
    'a two-factor account and a blank password' => [:two_factor, '']
  }.each do |name, (account, password)|
    test "#{name} costs exactly one bcrypt computation" do
      assert_equal(1, bcrypt_computations { sign_in_with(account, password) })
    end

    test "#{name} costs exactly one bcrypt computation while the account is locked" do
      [@plain, @two_factor].each(&:lock_access!)
      assert_equal(1, bcrypt_computations { sign_in_with(account, password) })
    end
  end

  # The one computation must not cost the second check its side effects: the wrong password
  # still counts against the account, and is answered as any other failed sign-in is.
  test "a two-factor account's wrong password still counts and fails like any other" do
    sign_in_with(:two_factor, 'wrong-password')
    assert_response :unprocessable_entity
    assert_equal 1, @two_factor.reload.failed_attempts
    plain_body = response.body.gsub(/(value|content|nonce)="[^"]*"/, '')

    sign_in_with(:plain, 'wrong-password')
    assert_response :unprocessable_entity
    assert_equal plain_body, response.body.gsub(/(value|content|nonce)="[^"]*"/, '')
  end

  test "a two-factor account's fifth wrong password locks it" do
    Devise.maximum_attempts.times { sign_in_with(:two_factor, 'wrong-password') }
    assert @two_factor.reload.access_locked?
  end

  # Devise's strategy never reaches the row for a blank password; neither does this path.
  test 'a blank password counts against no account' do
    sign_in_with(:two_factor, '')
    sign_in_with(:plain, '')
    assert_equal [0, 0], [@two_factor.reload.failed_attempts, @plain.reload.failed_attempts]
  end

  private

  def sign_in_with(account, password)
    email = account == :unknown ? 'nobody@example.test' : instance_variable_get("@#{account}").email
    post '/login', params: { user: { email: email, password: password } }
  end

  def bcrypt_computations
    BcryptComputationCounter.count = 0
    yield
    BcryptComputationCounter.count
  ensure
    BcryptComputationCounter.count = nil
  end
end
