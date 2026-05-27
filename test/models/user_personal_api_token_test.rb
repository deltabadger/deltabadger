# frozen_string_literal: true

require 'test_helper'

class UserPersonalApiTokenTest < ActiveSupport::TestCase
  setup do
    @user = create(:user)
  end

  teardown do
    Doorkeeper::AccessToken.delete_all
    Doorkeeper::Application.delete_all
  end

  # ---- shape ---------------------------------------------------------------

  test 'personal_api_token returns an :api-scoped AccessToken owned by the user' do
    token = @user.personal_api_token

    assert_instance_of Doorkeeper::AccessToken, token
    assert_equal @user.id, token.resource_owner_id
    assert_equal 'api', token.scopes.to_s
    assert_nil token.revoked_at
    assert_nil token.expires_in
  end

  test 'personal_api_token is idempotent — repeated calls return the same record' do
    first = @user.personal_api_token
    second = @user.reload.personal_api_token

    assert_equal first.id, second.id
    assert_equal first.token, second.token
  end

  test 'creating the token creates exactly one personal application owned by the user' do
    token = @user.personal_api_token
    app = token.application

    assert app.personal_access_token
    assert_equal @user.id, app.personal_owner_id
    assert_equal 1, Doorkeeper::Application.where(personal_owner_id: @user.id,
                                                  personal_access_token: true).count
  end

  # ---- regenerate ----------------------------------------------------------

  test 'regenerate_personal_api_token! revokes the old token and returns a new one with a different token string' do
    old_token = @user.personal_api_token
    old_token_string = old_token.token

    new_token = @user.regenerate_personal_api_token!

    assert_not_equal old_token_string, new_token.token
    assert_not_equal old_token.id, new_token.id

    old_token.reload
    assert old_token.revoked?, 'old token must be marked revoked after regenerate'
  end

  test 'after regenerate, exactly one active (revoked_at IS NULL) token exists for the user personal app' do
    @user.personal_api_token
    @user.regenerate_personal_api_token!
    @user.regenerate_personal_api_token!

    app = @user.personal_api_application
    active_count = Doorkeeper::AccessToken
                   .where(application_id: app.id, resource_owner_id: @user.id, revoked_at: nil)
                   .count
    assert_equal 1, active_count
  end

  test 'regenerate reuses the same personal application — no new app row is created' do
    original_app = @user.personal_api_token.application
    @user.regenerate_personal_api_token!
    @user.regenerate_personal_api_token!

    assert_equal 1, Doorkeeper::Application.where(personal_owner_id: @user.id,
                                                  personal_access_token: true).count
    assert_equal original_app.id, @user.reload.personal_api_application.id
  end

  # ---- DB-level guarantees -------------------------------------------------

  test 'partial unique index prevents a second personal app for the same user' do
    @user.personal_api_token # creates the first one

    # Bypass validations + populate Doorkeeper's auto-generated columns
    # explicitly so the row clears NOT NULL checks and reaches the partial
    # unique index. The constraint that must fire here is the index.
    dup = Doorkeeper::Application.new(
      name: 'duplicate personal app',
      redirect_uri: 'https://localhost/dup',
      confidential: false,
      scopes: 'api',
      personal_access_token: true,
      personal_owner_id: @user.id,
      uid: SecureRandom.hex(16),
      secret: SecureRandom.hex(16)
    )
    assert_raises(ActiveRecord::RecordNotUnique) { dup.save!(validate: false) }
  end

  test 'partial unique index does NOT constrain non-personal (DCR) apps' do
    # Multiple DCR apps for the same user (e.g. registered separately) must be allowed.
    Doorkeeper::Application.create!(
      name: 'DCR client 1', redirect_uri: 'http://x/cb1', confidential: false, scopes: 'api'
    )
    Doorkeeper::Application.create!(
      name: 'DCR client 2', redirect_uri: 'http://x/cb2', confidential: false, scopes: 'api'
    )
    # Both rows exist — no constraint violation.
    assert_equal 2, Doorkeeper::Application.where(personal_access_token: false).count
  end

  # ---- concurrency (best-effort with SQLite) -------------------------------

  test 'two concurrent ensure_personal_api_token! calls converge on exactly one app and one active token' do
    # SQLite + threads is flaky; the load-bearing assertion is the
    # DB-constraint test above. This test simulates the race using two
    # threads with a barrier. If SQLite locks bite, the rescue path in
    # create_personal_api_app_safely! catches the loser.
    barrier = Concurrent::CyclicBarrier.new(2)
    threads = 2.times.map do
      Thread.new do
        barrier.wait
        ActiveRecord::Base.connection_pool.with_connection do
          User.find(@user.id).ensure_personal_api_token!
        end
      end
    end
    threads.each(&:join)

    assert_equal 1, Doorkeeper::Application.where(personal_owner_id: @user.id,
                                                  personal_access_token: true).count
    app = @user.reload.personal_api_application
    active_count = Doorkeeper::AccessToken
                   .where(application_id: app.id, resource_owner_id: @user.id, revoked_at: nil)
                   .count
    assert_equal 1, active_count
  end

  # ---- isolation from mcp_applications -------------------------------------

  test 'personal app does NOT appear in user.mcp_applications' do
    @user.personal_api_token # creates personal app + token

    assert_empty @user.reload.mcp_applications.to_a
  end

  test 'a DCR-registered third-party app DOES appear in user.mcp_applications' do
    third_party = Doorkeeper::Application.create!(
      name: 'Claude Desktop', redirect_uri: 'http://localhost/cb',
      confidential: false, scopes: 'mcp'
    )
    Doorkeeper::AccessToken.create!(
      application: third_party, resource_owner_id: @user.id,
      token: SecureRandom.hex(32), scopes: 'mcp', expires_in: 3600
    )
    # And also a personal token coexists; it must not pollute the list.
    @user.personal_api_token

    apps = @user.reload.mcp_applications.to_a
    assert_includes apps, third_party
    assert_not_includes apps, @user.personal_api_application
  end

  # ---- cascade on User#destroy --------------------------------------------

  test 'User#destroy deletes the personal application and all its access tokens' do
    token = @user.personal_api_token
    app = token.application
    app_id = app.id
    token_id = token.id

    @user.destroy

    assert_nil Doorkeeper::Application.find_by(id: app_id),
               'personal application must be gone after user destroy'
    assert_nil Doorkeeper::AccessToken.find_by(id: token_id),
               'personal token must be gone after user destroy'
  end

  test 'destroying a user does not touch a different user\'s personal app/token' do
    other = create(:user)
    @user.personal_api_token
    keeper_token = other.personal_api_token
    keeper_app_id = keeper_token.application_id

    @user.destroy

    assert Doorkeeper::Application.exists?(id: keeper_app_id)
    assert Doorkeeper::AccessToken.exists?(id: keeper_token.id)
  end
end
