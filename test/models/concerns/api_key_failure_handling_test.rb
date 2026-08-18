require 'test_helper'

# The tracker's failure flash is the only thing standing between a user and guessing why their
# exchange stopped syncing (issue #153), so what it says — and what it does to the key — is pinned
# per failure class here.
class ApiKeyFailureHandlingTest < ActiveSupport::TestCase
  class Host
    include ApiKeyFailureHandling

    public :handle_api_key_failure
  end

  setup do
    @user = create(:user)
    @kraken = create(:kraken_exchange)
    @api_key = create(:api_key, user: @user, exchange: @kraken)
    @host = Host.new
  end

  test 'a missing permission does not condemn the key' do
    expect_broadcast(message: 'EGeneral:Permission denied', reason: :permission)

    fail_with('EGeneral:Permission denied')

    assert_equal 'correct', @api_key.reload.status,
                 'a key that is merely missing a scope must stay usable — :incorrect drops it from ' \
                 'every correct-scoped sync, and re-entering the same credentials cannot add a permission'
  end

  test 'an invalid key is still condemned' do
    expect_broadcast(message: 'EAPI:Invalid key', reason: :invalid)

    fail_with('EAPI:Invalid key')

    assert_equal 'incorrect', @api_key.reload.status
  end

  test 'a transient failure neither condemns the key nor advises replacing it' do
    expect_broadcast(message: I18n.t('errors.exchange.transient_nonce', exchange: 'Kraken'),
                     reason: :transient)

    fail_with('EAPI:Invalid nonce')

    assert_equal 'correct', @api_key.reload.status
  end

  test 'an unclassified failure falls through to the generic reason' do
    expect_broadcast(message: 'EGeneral:Something new', reason: :failed)

    fail_with('EGeneral:Something new')

    assert_equal 'correct', @api_key.reload.status
  end

  test 'the message is humanized rather than passed through raw' do
    expect_broadcast(message: 'Kraken restricts trading USDT in DE', reason: :failed)

    fail_with('EAccount:Invalid permissions:USDT trading restricted for DE.')
  end

  # AccountBalance::SyncAllJob is a scheduled fan-out: nothing sets I18n.locale, so without an
  # explicit wrapper every user on the instance would get this flash in the default locale.
  test 'the broadcast renders in the key owner locale, humanized message included' do
    @user.update!(locale: 'pl')
    polish = I18n.t('errors.exchange.regional_restriction',
                    exchange: 'Kraken', asset: 'USDT', country: 'DE', locale: :pl)
    assert_not_equal 'Kraken restricts trading USDT in DE', polish, 'fixture must differ per locale'
    expect_broadcast(message: polish, reason: :failed, capability: :balances)

    fail_with('EAccount:Invalid permissions:USDT trading restricted for DE.', capability: :balances)

    assert_equal :en, I18n.locale, 'the wrapper must not leak the user locale into the caller'
  end

  # THE regression pin for a 401. It is enough to SAY the venue rejected us, never enough to condemn
  # a stored key: Coinbase signs every request with a two-minute JWT built from local time, so clock
  # skew 401s a perfectly good key, and our own authenticated exchange proxy answers a wrong password
  # with 401 too. Condemning drops the key from every :correct-scoped sync until the user pastes new
  # credentials that were never the problem — so condemnation wants the venue's own words.
  test 'a bare 401 never condemns a stored key' do
    coinbase = create(:coinbase_exchange)
    key = create(:api_key, user: @user, exchange: coinbase)
    Turbo::StreamsChannel.stubs(:broadcast_append_to)

    @host.handle_api_key_failure(key, Result::Failure.new('Unauthorized', data: { status: 401 }),
                                 capability: :balances)

    assert_equal 'correct', key.reload.status
  end

  # Alpaca's two observed rejection bodies ARE listed as strings, which is what makes its revoked key
  # condemnable — the venue's own words, not a status.
  test 'a venue that names the rejection still condemns the key' do
    alpaca = create(:alpaca_exchange)
    key = create(:api_key, user: @user, exchange: alpaca)
    Turbo::StreamsChannel.stubs(:broadcast_append_to)

    @host.handle_api_key_failure(key, Result::Failure.new('unauthorized.', data: { status: 401 }),
                                 capability: :balances)

    assert_equal 'incorrect', key.reload.status
  end

  test 'nothing is broadcast for a successful result' do
    Turbo::StreamsChannel.expects(:broadcast_append_to).never

    @host.handle_api_key_failure(@api_key, Result::Success.new(1), capability: :transactions)
  end

  private

  def fail_with(message, capability: :transactions)
    @host.handle_api_key_failure(@api_key, Result::Failure.new(message), capability: capability)
  end

  def expect_broadcast(message:, reason:, capability: :transactions)
    Turbo::StreamsChannel.expects(:broadcast_append_to).with(
      "user_#{@user.id}", :sync,
      target: 'flash',
      partial: 'tracker/sync_key_error',
      locals: { exchange_name: 'Kraken', message: message, reason: reason, capability: capability }
    )
  end
end
