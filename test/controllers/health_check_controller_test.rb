require 'test_helper'

class HealthCheckControllerTest < ActionDispatch::IntegrationTest
  # The Docker HEALTHCHECK curls /health-check every 30s. It is a *liveness* probe:
  # it must report "the process is up", never depend on the app DB or the AR
  # connection pool. When the in-Puma stock-sync job pins the single `primary`
  # connection during its DB-write phase, a DB-touching probe times out
  # (ConnectionTimeoutError → 500).
  test 'performs no database queries' do
    # An admin must exist so ApplicationController#redirect_to_setup_if_needed does NOT
    # redirect to setup (which, pre-change, would 302 for an unrelated reason and mask
    # what we're testing). With an admin present and nobody signed in, the ONLY query
    # the inherited before_action issues is the unwanted `User.exists?(admin: true)` —
    # exactly the DB touch this test guards against.
    create(:user, admin: true)

    queries = []
    counter = lambda do |_name, _start, _finish, _id, payload|
      queries << payload[:sql] unless payload[:name] == 'SCHEMA'
    end

    ActiveSupport::Notifications.subscribed(counter, 'sql.active_record') do
      get health_check_path
    end

    assert_response :success
    assert_equal({ 'health' => 'check' }, JSON.parse(response.body))
    assert_empty queries, "health-check must not touch the database; got: #{queries.inspect}"
  end
end
