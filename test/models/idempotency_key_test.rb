# frozen_string_literal: true

require 'test_helper'

class IdempotencyKeyTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
  end

  test 'is valid with required attributes' do
    record = IdempotencyKey.new(
      user: @user, key: 'abc', request_fingerprint: 'fp1',
      state: 'in_progress', locked_at: Time.current
    )
    assert record.valid?
  end

  test 'state enum exposes in_progress and completed' do
    record = IdempotencyKey.create!(
      user: @user, key: 'abc', request_fingerprint: 'fp1',
      state: 'in_progress', locked_at: Time.current
    )
    assert record.in_progress?
    record.update!(state: 'completed', response_status: 201, response_body: '{"ok":true}')
    assert record.completed?
  end

  test 'enforces uniqueness on (user_id, key) at the DB layer' do
    # The validator catches the everyday duplicate. But the concern's
    # race-condition path relies on the DB-level constraint also raising,
    # so verify both: validation first, and a constraint violation when
    # validations are bypassed (simulating the lost-race case).
    IdempotencyKey.create!(user: @user, key: 'dup', request_fingerprint: 'fp1',
                           state: 'in_progress', locked_at: Time.current)

    assert_raises(ActiveRecord::RecordInvalid) do
      IdempotencyKey.create!(user: @user, key: 'dup', request_fingerprint: 'fp2',
                             state: 'in_progress', locked_at: Time.current)
    end

    racer = IdempotencyKey.new(user: @user, key: 'dup', request_fingerprint: 'fp3',
                               state: 'in_progress', locked_at: Time.current)
    assert_raises(ActiveRecord::RecordNotUnique) { racer.save!(validate: false) }
  end

  test 'allows the same key across different users (per-user scope)' do
    other = create(:user)
    IdempotencyKey.create!(user: @user, key: 'shared', request_fingerprint: 'fp1',
                           state: 'in_progress', locked_at: Time.current)
    assert_nothing_raised do
      IdempotencyKey.create!(user: other, key: 'shared', request_fingerprint: 'fp2',
                             state: 'in_progress', locked_at: Time.current)
    end
  end
end
