# Loaded by pages.rb's figures command. Only harnesses change; Rails remains the oracle.
FIGURES_LIBRARY = true
require_relative 'figures'
module PageFigures
  class << self
    attr_accessor :broadcasts
  end

  module_function

  NAMES = %w[no_orders basket_buys row_readings smart_slices index_rotation liquidations tax_lots old_rows sold_out crypto_basket split
             split_fresh split_unsized price_untraded].freeze
  # Pair bots (Bots::DcaSingleAsset, Bots::Signal) from figures.rb. Not here: a pair whose figures raise in Rails (no
  # fragment to render), the D1/D2 row vectors (library divergences, tests/figures.rs), KuCoin's stepped markets and a
  # tether quote (this grid holds no conversion: tests/figures.rs and figure_fx.rs cover them).
  PAIRS = %w[account_pair_bot pair_buys pair_crypto pair_sell_excess signal_sell_only signal_first_sell pair_tiny_avg pair_no_orders
             pair_submitted_unfilled pair_split_settled pair_split_quarantine pair_unresolved_split pair_no_ticker pair_tickerless_two_spellings
             pair_tickerless_null_base pair_delisted pair_replaced_listing pair_price_refused pair_candles_failed
             pair_display_raised pair_restating pair_row_zero_proceeds pair_row_open_partial].freeze
  # The scenarios whose bot pages Rust serves from its own routes (page_figures_grid.rs). Of the pairs, only a signal bot's page
  # is served: each pair scenario framed is recorded again as a signal bot (`signal_framed`).
  FRAMED = %w[no_orders basket_buys hidden owner_account owner_candles_failed signal_sell_only framed_signal_buys framed_signal_candles_failed
              framed_signal_price_refused].freeze

  # The dashboard, then each bot page and its chart frame, through Rails' own controllers, as a browser loads them: the
  # frame at the src its page gave it. Recorded cold, before any figure is cached, and warm, once they are.
  def pages(user, bots)
    Warden.test_mode!
    session = ActionDispatch::Integration::Session.new(Rails.application)
    session.host! 'localhost:3000'
    heard = PageFigures.broadcasts
    PageFigures.broadcasts = nil # a page load broadcasts nothing; whatever it might is not the figures' own
    # On a thread of its own, as a server's request would be: the app's executor leaves the runner's own state alone.
    Thread.new do
      { 'dashboard' => (page(session, user, '/bots') if bots.size > 1), # a lone bot's dashboard is its page
        'bots' => bots.to_h do |bot|
          body = page(session, user, "/bots/#{bot.id}")
          # A page whose chart is ready draws it inline: there is no frame to load.
          src = Nokogiri::HTML(body).at_css('turbo-frame#bot_chart')['src']
          [bot.id, { 'page' => body, 'chart' => (page(session, user, src, 'Turbo-Frame' => 'bot_chart') if src) }]
        end }
    end.value
  ensure
    PageFigures.broadcasts = heard
    Warden.test_reset!
  end

  def page(session, user, path, headers = {})
    Warden.on_next_request { |proxy| proxy.set_user(user, scope: :user) }
    session.get(path, headers:, env: { 'action_dispatch.show_exceptions' => :none }) # an error raises with its own message
    raise "#{path}: #{session.response.status}" unless session.response.status == 200

    session.response.body
  end

  # The owner's account: a basket, a pair bot and an index, on the hosted provider.
  def owner_account(list)
    owner = list.find { |s| s['name'] == 'index_rotation' }.deep_dup
    owner['name'] = 'owner_account'
    owner['provider'] = 'deltabadger'
    owner.delete('delist')
    index = owner['bots'].first
    index['orders'].reject! { |order| order['sym'] == 'LLL' }
    index['exited'].delete('LLL')
    basket = list.find { |s| s['name'] == 'basket_buys' }['bots'].first.deep_dup
    basket['settings'] = { 'limit_ordered' => true, 'smart_intervaled' => true, 'smart_interval_quote_amount' => 10.0 }
    single = list.find { |s| s['name'] == 'row_readings' }['bots'].first.deep_dup
    # Priced owner control; row_readings separately asserts the NULL-fill refusal.
    single['orders'].reject! { |order| order['price'].nil? || order['price'].to_f <= 0 }
    single['orders'].each do |order|
      quantity = order['amount_exec'] || (order.fetch('ext', 'closed') == 'closed' ? order['amount'] : nil)
      order['quote_amount_exec'] = (order['price'].to_d * quantity.to_d).to_s('F') if quantity.to_d.positive? && !order['quote_amount_exec'].to_d.positive?
    end
    single['settings']['limit_ordered'] = true
    owner['bots'] = [basket, single, index]
    owner
  end

  # The account's figures in the currency it is shown in (Denomination), from figures.rb's three-bot account on the
  # hosted provider: a USD basket, a USDT basket, an empty one, a deleted and an archived one.
  def display_currency(list)
    account = Figures.scenarios.find { |s| s['name'] == 'account_deltabadger' } or raise 'missing account_deltabadger'
    rates = "#{Figures::HOSTED}/exchange_rates"
    prices = "#{Figures::HOSTED}/prices?coin_ids=tether&vs_currencies=usd"
    script = account['script'].deep_dup
    { 'gbp' => 47_481.4, 'chf' => 51_210.3 }.each { |code, value| script[rates]['body']['data'][code] = { 'name' => 'x', 'unit' => 'x', 'value' => value, 'type' => 'fiat' } }
    down = script.merge(rates => Figures::SERVER_ERROR, prices => Figures::SERVER_ERROR)
    variants = {
      'fx_eur' => {}, 'fx_gbp' => { 'display_currency' => 'GBP' }, 'fx_chf' => { 'display_currency' => 'CHF' },
      'fx_usdt_usd' => { 'display_currency' => 'USD' }, 'fx_hidden_eur' => { 'hide_balances' => true },
      'fx_de' => { 'locale' => 'de' }, 'fx_rates_down' => { 'script' => down }
    }
    out = variants.map { |name, change| account.deep_dup.merge('script' => script).merge(change).merge('name' => name) }
    # Dollars only, shown in euros, with no rate to be had: Denomination.for's fallback.
    usd = list.find { |s| s['name'] == 'basket_buys' }.deep_dup
    out << usd.merge('name' => 'fx_display_down', 'provider' => 'deltabadger', 'display_currency' => 'EUR', 'script' => usd['script'].merge(rates => Figures::SERVER_ERROR))
    out
  end

  # Each bot's tile, metrics panel and chart as Rails renders them from what its caches hold (computed where they hold nothing).
  def bot_fragments(user, bots)
    bots.to_h do |bot|
      data = bot.metrics_with_current_prices
      marked = bot.metrics_with_current_prices_and_candles
      render = ->(partial, locals) { ApplicationController.render(partial:, locals:) }
      profit = bot.profit_in_usd(data, cache_only: false) unless user.hide_balances?
      pair = bot.dca_single_asset? || bot.signal?
      [bot.id, {
        'tile' => render.call('bots/bot_tile/bot_tile_pnl',
                              { bot:, pnl: data[:pnl], profit_usd: profit, denomination: user.denomination, loading: false }),
        'metrics' => if pair
                       render.call('bots/dca_single_assets/metrics', { bot:, metrics: data, loading: false })
                     else
                       render.call('bots/composition/metrics', { bot:, metrics: data, loading: false, exited_title_key: bot.exited_title_key })
                     end,
        'chart' => render.call('bots/chart', { bot:, metrics: marked, loading: false, current_user: user })
      }]
    end
  end

  def account_fragment(user)
    snapshot = User::PnlHistory.snapshot(user, live: true)
    ApplicationController.render(partial: 'bots/global_pnl', locals: {
                                   global_pnl: user.global_pnl(use_cache: false), history: snapshot[:result], loading: false,
                                   hide_balances: user.hide_balances?, denomination: user.denomination
                                 })
  end

  def harness
    Rails.logger.level = :warn
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedMarket::Adapter)
    ActionCable.server.singleton_class.prepend(Module.new do
      def broadcast(stream, message, **)
        PageFigures.broadcasts << [stream, message] if PageFigures.broadcasts
      end
    end)
  end

  def scenario_list
    list = Figures.scenarios.select { |s| NAMES.include?(s['name']) }
    raise 'missing scenario' unless list.size == NAMES.size

    list
  end

  def pair_list
    list = Figures.scenarios.select { |s| PAIRS.include?(s['name']) }
    raise 'missing pair scenario' unless list.size == PAIRS.size

    pair = list.find { |s| s['name'] == 'pair_buys' }
    list << pair.merge('name' => 'pair_hidden', 'hide_balances' => true)
    hosted = Figures.scenarios.find { |s| s['name'] == 'account_deltabadger' }['script'].select { |key, _| key.include?(Figures::HOSTED) }
    list << pair.merge('name' => 'fx_pair_eur', 'provider' => 'deltabadger', 'display_currency' => 'EUR', 'script' => pair['script'].merge(hosted))
    list << pair.merge('name' => 'pair_locked')
    { 'framed_signal_buys' => 'pair_buys', 'framed_signal_candles_failed' => 'pair_candles_failed', 'framed_signal_price_refused' => 'pair_price_refused' }.each do |name, source|
      sc = list.find { |s| s['name'] == source }.deep_dup
      sc['bots'].each { |bot| bot['type'] = 'signal' }
      list << sc.merge('name' => name)
    end
    list
  end

  # What the owner's figures look like over time, a step at a time: what a page reads from Rails' caches as they stand
  # (`actual`), and a fresh computation (`fresh`): every cache layer cleared (the three metrics keys, the candle series,
  # the rates), rendered at `t_c`, the moment the figures being compared were computed. `t_r` is when the page reads.
  #   warm; a reread with no write; another bot started; an order filled (Bot::UpdateMetricsJob and its broadcast); a reread; the cut; the
  #   provider failing at the next cut.
  # `writes`: the rows a step committed, as stored. The provider failure's `script` replaces the market's from its step on.
  #   Recorded for the owner's account (a basket's buy fills) and for a signal bot that only sells (a signal call fills).
  def chronology(root)
    harness
    template = Dir.mktmpdir('page-figures')
    assets = Figures.world(template)
    chronicle(root, template, assets, owner_account(scenario_list), 'buy')
    chronicle(root, template, assets, pair_list.find { |s| s['name'] == 'signal_sell_only' }, 'sell')
  ensure
    FileUtils.rm_rf(template) if template
  end

  def chronicle(root, template, assets, sc, side)
    dir = File.join(root, sc['name'])
    FileUtils.mkdir_p(dir)
    user, bots = Figures.build(sc.merge('dir' => dir), template, assets)
    user.update_columns(wash_sale_enabled: false)
    Rails.cache.clear
    ScriptedMarket.http = sc['script']
    ScriptedMarket.requests = []
    ScriptedMarket.gaps = []
    ActiveRecord::Base.connection.execute("VACUUM INTO '#{File.join(dir, 'initial.sqlite3')}'")
    t0 = Figures.time(sc['at'])
    cut = Time.at((t0.to_i / 300 * 300) + 300).utc
    t3 = t0 + 120
    failing = sc['script'].to_h { |key, reply| [key, key.match?(%r{/snapshots|/latest/trades}) ? { 'status' => 503, 'body' => {} } : reply] }
    reload = -> { [User.find(user.id), bots.map { |bot| Bot.find(bot.id) }] }
    read = lambda do |at|
      reader, readers = reload.call
      Figures.travel_to(at, with_usec: true) { { 'bots' => bot_fragments(reader, readers), 'account' => account_fragment(reader) } }
    end
    fresh = lambda do |at|
      kept = Rails.cache
      Rails.cache = ActiveSupport::Cache::MemoryStore.new
      read.call(at)
    ensure
      Rails.cache = kept
    end
    steps = []
    step = lambda do |name, t_r, t_c, **rest|
      steps << { 'name' => name, 't_r' => t_r.utc.iso8601(9), 't_c' => t_c.utc.iso8601(9), 'actual' => read.call(t_r), 'fresh' => fresh.call(t_c) }.merge(rest.stringify_keys)
    end
    step.call('warm', t0, t0)
    step.call('reread', t0 + 90, t0)
    basket = bots.first
    # Another bot is started: its row is the only write, and every other bot's cache is read as it was (BotsController#index
    # reads each bot's own key). It has no order, so the headline still sums the others (User#global_pnl_snapshot).
    started_at = t0 + 100
    started = basket.attributes.except('id').merge('label' => 'Started later', 'status' => Bot.statuses['scheduled'],
                                                   'created_at' => started_at, 'updated_at' => started_at)
    started_id = Bot.insert!(started).rows.first&.first || Bot.find_by!(label: 'Started later').id
    started_row = ActiveRecord::Base.connection.select_one("SELECT * FROM bots WHERE id = #{started_id.to_i}")
    step.call('started', started_at, t0, writes: [['bots', started_row]])
    row = { 'bot_id' => basket.id, 'exchange_id' => basket.exchange_id, 'base_asset_id' => assets['AAA'], 'quote_asset_id' => basket.settings['quote_asset_id'],
            'base' => 'AAA', 'quote' => 'USD', 'side' => side, 'transaction_type' => 'REGULAR', 'status' => 'submitted', 'external_status' => 'closed',
            'external_id' => 'chronology-fill', 'order_type' => 'market_order', 'price' => '104.5', 'amount' => '0.2', 'amount_exec' => '0.2',
            'quote_amount_exec' => '20.9', 'bot_interval' => 'week', 'bot_quote_amount' => 100.0, 'error_messages' => [], 'created_at' => t3, 'updated_at' => t3 }
    id = Transaction.insert!(row).rows.first&.first || Transaction.find_by!(external_id: 'chronology-fill').id
    written = ActiveRecord::Base.connection.select_one("SELECT * FROM transactions WHERE id = #{id.to_i}")
    PageFigures.broadcasts = []
    adapter = ActiveJob::Base.queue_adapter
    ActiveJob::Base.queue_adapter = :test
    Figures.travel_to(t3, with_usec: true) do
      Bot::UpdateMetricsJob.perform_now(Bot.find(basket.id))
      Bot::BroadcastMetricsUpdateJob.perform_now(Bot.find(basket.id))
    end
    ActiveJob::Base.queue_adapter = adapter
    step.call('filled', t3, t3, writes: [['transactions', written]], broadcasts: PageFigures.broadcasts)
    PageFigures.broadcasts = nil
    step.call('reread_filled', t3 + 60, t3)
    step.call('cut', cut + 5, cut + 5)
    ScriptedMarket.http = failing
    step.call('provider_failure', cut + 305, cut + 305, script: failing)
    raise "unscripted market: #{ScriptedMarket.gaps.uniq}" unless ScriptedMarket.gaps.empty?

    File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate({ 'parity_scratch' => true, 'at' => t0.utc.iso8601(9), 'user_id' => user.id,
                                                                        'bot_ids' => bots.map(&:id), 'script' => sc['script'] }))
    File.write(File.join(dir, 'chronology.json'), JSON.pretty_generate({ 'w1' => %w[filled reread_filled], 'steps' => steps }))
    ActiveRecord::Base.connection_pool.disconnect!
  end

  # Denomination#format at the edges of its sign: number_to_currency writes a loss in the negative layout only when it is at
  # least half of the last digit shown. (currency, USD -> it rate, USD amount, precision) -> what Rails writes.
  def boundaries(root)
    rates = { 'USD' => '1', 'EUR' => '0.859375', 'CHF' => '0.8001609375000001', 'PLN' => '3.6876796875' }
    amounts = %w[-0.5 -0.4999 -0.49 -0.005 -0.0049 -0.004 -0.5000001 -1.5 -2.5 -0.0 0.004 0.5 -581.8181818 -0.58181818]
    cases = rates.flat_map do |currency, rate|
      amounts.product([0, 2]).map do |usd, precision|
        [currency, rate, usd, precision, Denomination.new(currency, BigDecimal(rate)).format(BigDecimal(usd), precision:).to_s]
      end
    end
    FileUtils.mkdir_p(File.join(root, 'boundaries'))
    File.write(File.join(root, 'boundaries', 'rails.json'), JSON.pretty_generate(cases))
  end

  def run(root)
    boundaries(root)
    harness
    template = Dir.mktmpdir('page-figures')
    assets = Figures.world(template)
    list = scenario_list

    list << list.find { |s| s['name'] == 'basket_buys' }.merge('name' => 'hidden', 'hide_balances' => true)
    %w[locked locked_sold locked_gb locked_sold_gb locked_blank locked_whitespace locked_unicode stranded_offset no_composition].each do |name|
      source = name.start_with?('locked_sold') ? 'sold_out' : 'basket_buys'
      list << list.find { |s| s['name'] == source }.merge('name' => name)
    end
    list << owner_account(list)
    # A failure stays with what it failed: one holding's candles (Rails rescues each symbol's fetch), and the account's other
    # figures stand. Failed prices are the chronology's provider_failure step.
    owner = list.last
    bars = owner['script'].keys.grep(%r{/v2/stocks/\w+/bars}).first[%r{/v2/stocks/(\w+)/bars}, 1]
    list << owner.deep_dup.merge('name' => 'owner_candles_failed',
                                 'script' => owner['script'].to_h { |key, reply| [key, key.include?("/v2/stocks/#{bars}/bars") ? Figures::SERVER_ERROR : reply] })
    # An index bot's liquidation, redeploy and rebalance legs: the bot page (GET /bots/:id) is compared on it too.
    specials = list.find { |s| s['name'] == 'liquidations' }.deep_dup
    specials['name'] = 'index_specials'
    specials['provider'] = 'deltabadger'
    rebalanced = Figures::T0 + (9 * Figures::D) + 60
    specials['bots'].first['orders'] += [Figures.sell('AAA', rebalanced, '53', '0.1', type: 'REBALANCE'),
                                         Figures.buy('DDD', rebalanced + 1, '90.1', '0.058823529', type: 'REBALANCE')]
    list << specials
    list.concat(display_currency(list))
    list.concat(pair_list)
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      user, bots = Figures.build(sc.merge('dir' => dir), template, assets)
      # A pair bot memoizes its ticker: read afresh, as a request does, after the scenario's own writes.
      bots = bots.map { |bot| Bot.find(bot.id) } if bots.any? { |bot| bot.dca_single_asset? || bot.signal? }
      user.update_columns(wash_sale_enabled: false)
      if sc['name'].start_with?('locked') || sc['name'] == 'pair_locked'
        jurisdiction = sc['name'].end_with?('_gb') ? 'GB' : 'US'
        jurisdiction = { 'locked_blank' => '', 'locked_whitespace' => " \t\n", 'locked_unicode' => "\u00a0\u3000" }.fetch(sc['name'], jurisdiction)
        user.update_columns(wash_sale_enabled: true, wash_sale_jurisdiction: jurisdiction)
        user.wash_sale_locks.create!(asset_id: assets['AAA'], buy_locked_until: Figures.time(sc['at']) + 10.days, source: 'ledger')
      end
      bots.first.update_columns(redeploy_declined_offset: 100) if sc['name'] == 'stranded_offset'
      bots.first.bot_index_assets.delete_all if sc['name'] == 'no_composition'
      user.update_columns(locale: sc['locale']) if sc['locale']
      Rails.cache.clear
      ScriptedMarket.http = sc['script']
      ScriptedMarket.requests = []
      ScriptedMarket.gaps = []
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate({
                                                                         'parity_scratch' => true, 'at' => Figures.time(sc['at']).utc.iso8601(9),
                                                                         'user_id' => user.id, 'bot_ids' => bots.map(&:id), 'script' => sc['script']
                                                                       }))
      PageFigures.broadcasts = []
      before_offset = bots.first.reload.redeploy_declined_offset
      out = {}
      Figures.travel_to(Figures.time(sc['at']), with_usec: true) do
        out['cold'] = pages(user, bots) if FRAMED.include?(sc['name'])
        out['bots'] = bot_fragments(user, bots)
        bots.each do |bot|
          bot.broadcast_pnl_update
          if bot.respond_to?(:broadcast_metrics_panel)
            bot.broadcast_metrics_panel
            bot.broadcast_chart
          else
            bot.broadcast_metrics_update # a pair bot: its panel, then its chart
          end
        end
        user.broadcast_global_pnl_update
        out['account'] = account_fragment(user)
        out['warm'] = pages(user, bots) if FRAMED.include?(sc['name'])
      end
      raise 'unscripted market' unless ScriptedMarket.gaps.empty?

      out['broadcasts'] = PageFigures.broadcasts
      out['offset'] = [before_offset.to_s, bots.first.reload.redeploy_declined_offset.to_s]
      bots.first.update_columns(redeploy_declined_offset: before_offset) # give Rust the same input Rails read
      out['requests'] = ScriptedMarket.requests.uniq.sort
      File.write(File.join(dir, 'rails.json'), JSON.pretty_generate(out))
      ActiveRecord::Base.connection_pool.disconnect!
    end
  ensure
    FileUtils.rm_rf(template) if template
  end
end
