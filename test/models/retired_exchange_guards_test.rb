require 'test_helper'

# Nothing may start trading on a retired venue, and no key may be stored for one — no matter which
# door the request comes through. The bot path and the rule path need DIFFERENT triggers:
# Bot::Lifecycle#start runs valid?(:start), while Rules::Withdrawal#start calls a plain update!,
# which never enters the :start context. Both are covered by one validation.
class RetiredExchangeGuardsTest < ActiveSupport::TestCase
  setup do
    @retired = Exchanges::Bitmart.create!(name: 'Bitmart', available: false)
    @user = create(:user)
  end

  # ── bots ──────────────────────────────────────────────────────────────────

  test 'a bot on a retired exchange fails the start validation with a readable message' do
    bot = create(:dca_single_asset, user: @user, exchange: @retired)

    refute bot.valid?(:start)
    assert_includes bot.errors.full_messages.to_sentence, I18n.t('errors.exchange_retired')
  end

  test 'a bot on a live exchange still starts' do
    bot = create(:dca_single_asset, user: @user, exchange: create(:binance_exchange))

    assert bot.valid?(:start)
  end

  test 'a bot on a retired exchange can still be stopped and deleted' do
    # A bot that was already running when the venue died. It cannot be put INTO a working status
    # through the model (that is the guard above), so the pre-existing state is set the way the
    # world actually produced it — the bot was working before the retirement.
    bot = create(:dca_single_asset, user: @user, exchange: @retired)
    bot.update_column(:status, Bot.statuses[:scheduled])
    bot.reload

    assert bot.stop
    assert_predicate bot.reload, :stopped?
    assert bot.delete
    assert_predicate bot.reload, :deleted?
  end

  # ── rules ─────────────────────────────────────────────────────────────────

  test 'a withdrawal rule on a retired exchange cannot be moved into a working status' do
    rule = create_withdrawal_rule(@retired)

    assert_raises(ActiveRecord::RecordInvalid) { rule.start }
    refute_predicate rule.reload, :working?
  end

  test 'a withdrawal rule on a retired exchange can still be stopped' do
    rule = create_withdrawal_rule(@retired)

    rule.stop
    assert_predicate rule.reload, :stopped?
  end

  test 'a withdrawal rule on a live exchange still starts' do
    rule = create_withdrawal_rule(create(:binance_exchange))

    rule.start
    assert_predicate rule.reload, :scheduled?
  end

  # ── API keys ──────────────────────────────────────────────────────────────

  test 'an API key for a retired exchange cannot be persisted' do
    api_key = build(:api_key, user: @user, exchange: @retired)

    refute api_key.valid?
    assert_includes api_key.errors.full_messages.to_sentence, I18n.t('errors.exchange_retired')
  end

  test 'validate_credentials! never calls the retired exchange' do
    api_key = build(:api_key, user: @user, exchange: @retired)
    Exchanges::Bitmart.any_instance.expects(:get_api_key_validity).never

    api_key.validate_credentials!(key: 'k', secret: 's')

    refute_predicate api_key, :correct?
    refute_predicate api_key, :persisted?
  end

  # AddApiKey backs legacy POST /api/api_keys, where a bare create! would surface the new
  # validation as a 500 instead of a structured rejection.
  test 'AddApiKey returns a failure Result rather than raising on a retired exchange' do
    result = AddApiKey.call(user: @user, exchange: @retired, key_type: :trading,
                            key: 'k', secret: 's')

    assert_predicate result, :failure?
    assert_includes result.errors.to_sentence, I18n.t('errors.exchange_retired')
  end

  private

  def create_withdrawal_rule(exchange)
    Rules::Withdrawal.create!(
      user: @user,
      exchange: exchange,
      asset: create(:asset, :bitcoin),
      address: 'bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh',
      status: :stopped,
      settings: { 'withdrawal_percentage' => 100, 'max_fee_percentage' => 1, 'threshold_type' => 'max_fee' }
    )
  end
end
