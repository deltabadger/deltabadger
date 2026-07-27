require 'test_helper'

# The point of keeping a retired venue in the schema: a stranded bot stays visible, reads as
# disconnected, and can be moved to a live exchange without losing its trade history.
class RetiredExchangeSwitchTest < ActionDispatch::IntegrationTest
  setup do
    @user = create(:user, admin: true, setup_completed: true) # satisfies the onboarding gate
    sign_in @user

    @retired = Exchanges::Bitmart.create!(name: 'Bitmart', available: false)
    @base = create(:asset, :bitcoin)
    @quote = create(:asset, :usd)
    @bot = create(:dca_single_asset, :stopped, user: @user, exchange: @retired,
                                               base_asset: @base, quote_asset: @quote,
                                               with_api_key: false)
    @binance = create(:binance_exchange)
    create(:ticker, exchange: @binance, base_asset: @base, quote_asset: @quote)
  end

  test 'the bot page renders for a bot stranded on a retired exchange' do
    get bot_path(id: @bot.id)

    assert_response :success
    # The venue logo partial is keyed off name_id and must survive the removal.
    assert_match 'exchange--bitmart', response.body
  end

  test 'the retired venue offers no way to connect or reconnect a key' do
    get bot_path(id: @bot.id)

    assert_response :success
    refute_match new_bot_add_api_key_path(bot_id: @bot.id), response.body
    assert_match I18n.t('bot.status.exchange_retired'), response.body
  end

  test 'the switcher offers live exchanges that carry the pair' do
    get bot_path(id: @bot.id)

    assert_response :success
    assert_match @binance.name, response.body
  end

  test 'switching to a live exchange keeps the trade history' do
    history = create(:transaction, bot: @bot, exchange: @retired, status: :submitted,
                                   external_status: :closed, external_id: 'bm-hist')

    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { exchange_id: @binance.id } }

    assert_equal @binance.id, @bot.reload.exchange_id
    assert_equal @retired.id, history.reload.exchange_id, 'the old trade keeps the venue it happened on'
    assert_equal 'closed', history.external_status
  end

  # validate_unchangeable_exchange blocks a switch while transactions.waiting.any? — which is why
  # the migration flips unresolvable Bitmart orders to :abandoned rather than leaving them open.
  test 'an abandoned order does not block the switch' do
    create(:transaction, bot: @bot, exchange: @retired, status: :submitted,
                         external_status: :abandoned, external_id: 'bm-abandoned')

    patch bot_path(id: @bot.id), params: { bots_dca_single_asset: { exchange_id: @binance.id } }

    assert_equal @binance.id, @bot.reload.exchange_id
  end

  test 'the API-key wizard refuses a retired exchange instead of showing generic credential copy' do
    get new_bot_add_api_key_path(bot_id: @bot.id)

    assert_response :redirect
    assert_equal I18n.t('errors.exchange_retired'), flash[:alert]
  end
end
