# frozen_string_literal: true

require 'test_helper'

# The venue chip on the bot page. The menu always opens, because "there is nowhere to move this
# bot" is the answer the user came for. It used to render no menu at all in exactly those cases —
# a running bot, and a pair that exactly one venue lists — so the chip took the click, tinted the
# page and showed nothing, which reads as a control something else is blocking.
#
# The two dead ends are not the same dead end: one is undone by stopping the bot, the other never
# is, so each says which one it is.
class ExchangeSelectTest < ActionView::TestCase
  setup do
    # Routes are scoped by an optional (:locale), which a real request always fills in. Without it
    # the single positional argument of bot_path(bot) lands in that segment instead of :id.
    def @controller.default_url_options = { locale: I18n.default_locale }

    # Dry run hands every bot a `correct` key out of thin air, so Reconnect would always be on
    # offer and the empty menu could never happen. This partial is about a real disconnected bot.
    Rails.configuration.dry_run = false

    @base = create(:asset, symbol: 'UBTC', name: 'Unit Bitcoin', external_id: 'unit-bitcoin')
    @quote = create(:asset, :usdt)
    @hyperliquid = create(:hyperliquid_exchange)
  end

  teardown { Rails.configuration.dry_run = true }

  def render_select(bot)
    render partial: 'bots/exchange_select', locals: { bot: bot }
  end

  def bot_on(exchange, **attrs)
    create(:dca_single_asset, exchange: exchange, base_asset: @base, quote_asset: @quote,
                              with_api_key: false, **attrs)
  end

  def rival_venue
    create(:kraken_exchange).tap do |exchange|
      create(:ticker, exchange: exchange, base_asset: @base, quote_asset: @quote)
    end
  end

  test 'the only venue that lists the pair says so instead of opening an empty menu' do
    render_select bot_on(@hyperliquid)

    assert_select '.dropdown--exchanges .dropdown__item--note', /UBTC/
    assert_select '.dropdown--exchanges button', 0
  end

  test 'a running bot is told to stop, not that its assets are unlisted' do
    rival_venue
    render_select bot_on(@hyperliquid, status: :waiting)

    assert_select '.dropdown--exchanges .dropdown__item--note', 1
    assert_select '.dropdown--exchanges .dropdown__item--note', { text: /UBTC/, count: 0 }
    assert_select '.dropdown--exchanges button', 0
  end

  test 'a stopped bot with somewhere to go gets the venues, not a note' do
    rival_venue
    render_select bot_on(@hyperliquid, status: :stopped)

    assert_select '.dropdown--exchanges .dropdown__item--note', 0
    assert_select '.dropdown--exchanges button', 1
  end
end
