# frozen_string_literal: true

require 'test_helper'

# Drives the Idempotency concern via a stub controller mounted into the
# routes only for this test. Keeps the concern decoupled from any real
# REST endpoint (the plan lands this before any trade controller).
class IdempotencyConcernTest < ActionDispatch::IntegrationTest
  # ---- stub controller ----------------------------------------------------

  class StubController < ActionController::API
    include Idempotency

    class << self
      attr_accessor :call_count

      def reset!
        @call_count = 0
      end
    end

    before_action :set_stub_user

    def create
      idempotent_request do
        self.class.call_count += 1
        # Echo the body so tests can assert response storage shape.
        [201, { ok: true, echo: request.params['amount'], n: self.class.call_count }]
      end
    end

    def create_failure
      idempotent_request do
        self.class.call_count += 1
        [422, { error: { code: 'rejected_by_exchange', message: 'no funds' } }]
      end
    end

    private

    def set_stub_user
      @current_user = User.find(request.headers['X-Stub-User-Id'])
    end

    attr_reader :current_user
  end

  # Register the stub routes into the main app once. They live under a
  # `__test__` prefix so they cannot collide with real endpoints.
  Rails.application.routes.append do
    post '/__test__/idem/create', to: 'idempotency_concern_test/stub#create'
    post '/__test__/idem/fail', to: 'idempotency_concern_test/stub#create_failure'
  end
  Rails.application.reload_routes!

  setup do
    @user = create(:user)
    StubController.reset!
  end

  teardown do
    IdempotencyKey.delete_all
  end

  # ---- missing key --------------------------------------------------------

  test 'returns 400 idempotency_key_required when Idempotency-Key header is missing' do
    post '/__test__/idem/create', params: { amount: '10' },
                                  headers: { 'X-Stub-User-Id' => @user.id.to_s }, as: :json

    assert_response :bad_request
    json = JSON.parse(response.body)
    assert_equal 'idempotency_key_required', json['error']['code']
    assert_equal 0, StubController.call_count
    assert_equal 0, IdempotencyKey.count
  end

  test 'returns 400 when Idempotency-Key header is present but blank' do
    post '/__test__/idem/create', params: { amount: '10' }, headers: header(''), as: :json

    assert_response :bad_request
    assert_equal 'idempotency_key_required', JSON.parse(response.body)['error']['code']
  end

  # ---- first claim --------------------------------------------------------

  test 'first claim: runs the block, renders the response, stores the row as completed' do
    post '/__test__/idem/create', params: { amount: '10' }, headers: header('k1'), as: :json

    assert_response :created
    json = JSON.parse(response.body)
    assert_equal true, json['ok']
    assert_equal '10', json['echo']
    assert_equal 1, StubController.call_count

    record = IdempotencyKey.find_by!(user: @user, key: 'k1')
    assert record.completed?
    assert_equal 201, record.response_status
    assert_equal response.body, record.response_body
    assert record.request_fingerprint.present?
  end

  # ---- replay after completion --------------------------------------------

  test 'replay: same key + same fingerprint returns the stored response and does not re-run the block' do
    post '/__test__/idem/create', params: { amount: '10' }, headers: header('k1'), as: :json
    first_body = response.body
    first_status = response.status

    post '/__test__/idem/create', params: { amount: '10' }, headers: header('k1'), as: :json

    assert_equal first_status, response.status
    assert_equal first_body, response.body
    assert_equal 1, StubController.call_count, 'block must run only once'
  end

  test 'replay renders the stored body bytes verbatim (no JSON re-encoding)' do
    # First call to populate the row with the correct fingerprint, then
    # overwrite the stored body with a deliberately non-canonical JSON
    # string. If the concern rendered with `render json:` it would
    # re-encode (and the byte sequence would change). With `render body:`
    # the exact bytes survive.
    post '/__test__/idem/create', params: { amount: '10' }, headers: header('preseeded'), as: :json
    assert_response :created
    record = IdempotencyKey.find_by!(user: @user, key: 'preseeded')

    weird_body = %({  "ok":true,  "echo":"10"  })
    record.update_columns(response_body: weird_body)
    StubController.reset!

    post '/__test__/idem/create', params: { amount: '10' }, headers: header('preseeded'), as: :json

    assert_response :created
    assert_equal weird_body, response.body, 'replay must be byte-for-byte'
    assert_equal 0, StubController.call_count, 'block must not run on replay'
    assert_match(%r{application/json}, response.headers['Content-Type'])
  end

  # ---- same key, different fingerprint ------------------------------------

  test 'same key + different fingerprint returns 409 idempotency_key_reused' do
    post '/__test__/idem/create', params: { amount: '10' }, headers: header('k1'), as: :json
    assert_response :created

    post '/__test__/idem/create', params: { amount: '999' }, headers: header('k1'), as: :json

    assert_response :conflict
    assert_equal 'idempotency_key_reused', JSON.parse(response.body)['error']['code']
    assert_equal 1, StubController.call_count, 'block must not run on fingerprint mismatch'
  end

  # ---- in-progress conflict -----------------------------------------------

  test 'same key while another request is in_progress returns 409 idempotency_in_progress' do
    # Simulate a concurrent in-flight request by completing one request,
    # then flipping the row back to :in_progress (preserves the matching
    # fingerprint without exposing fingerprint internals to the test).
    post '/__test__/idem/create', params: { amount: '10' }, headers: header('inflight'), as: :json
    assert_response :created
    record = IdempotencyKey.find_by!(user: @user, key: 'inflight')
    record.update_columns(state: 'in_progress')
    StubController.reset!

    post '/__test__/idem/create', params: { amount: '10' }, headers: header('inflight'), as: :json

    assert_response :conflict
    assert_equal 'idempotency_in_progress', JSON.parse(response.body)['error']['code']
    assert_equal 0, StubController.call_count, 'block must not run while another request holds the claim'
  end

  # ---- failed responses are stored & replayed -----------------------------

  test 'failed exchange responses are stored and replayed (definitive outcome)' do
    post '/__test__/idem/fail', params: { amount: '10' }, headers: header('failkey'), as: :json
    assert_response :unprocessable_entity
    first_body = response.body

    post '/__test__/idem/fail', params: { amount: '10' }, headers: header('failkey'), as: :json

    assert_response :unprocessable_entity
    assert_equal first_body, response.body
    assert_equal 1, StubController.call_count, 'failure replay must not re-hit the exchange'
  end

  # ---- per-user isolation -------------------------------------------------

  test 'the same Idempotency-Key from a different user does not collide' do
    post '/__test__/idem/create', params: { amount: '10' }, headers: header('shared'), as: :json
    assert_response :created

    other = create(:user)
    post '/__test__/idem/create', params: { amount: '10' },
                                  headers: { 'Idempotency-Key' => 'shared', 'X-Stub-User-Id' => other.id.to_s },
                                  as: :json

    assert_response :created
    assert_equal 2, StubController.call_count, 'each user gets their own claim'
  end

  private

  def header(key)
    { 'Idempotency-Key' => key, 'X-Stub-User-Id' => @user.id.to_s }
  end
end
