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
  def run(root)
    Rails.logger.level = :warn
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedMarket::Adapter)
    ActionCable.server.singleton_class.prepend(Module.new do
      def broadcast(stream, message, **)
        PageFigures.broadcasts << [stream, message] if PageFigures.broadcasts
      end
    end)
    template = Dir.mktmpdir('page-figures')
    assets = Figures.world(template)
    list = Figures.scenarios.select { |s| NAMES.include?(s['name']) }
    raise 'missing scenario' unless list.size == NAMES.size

    list << list.find { |s| s['name'] == 'basket_buys' }.merge('name' => 'hidden', 'hide_balances' => true)
    %w[locked locked_sold locked_gb locked_sold_gb locked_blank locked_whitespace locked_unicode stranded_offset no_composition].each do |name|
      source = name.start_with?('locked_sold') ? 'sold_out' : 'basket_buys'
      list << list.find { |s| s['name'] == source }.merge('name' => name)
    end
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
    single['settings']['limit_ordered'] = true
    owner['bots'] = [basket, single, index]
    list << owner
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      user, bots = Figures.build(sc.merge('dir' => dir), template, assets)
      user.update_columns(wash_sale_enabled: false)
      if sc['name'].start_with?('locked')
        jurisdiction = sc['name'].end_with?('_gb') ? 'GB' : 'US'
        jurisdiction = { 'locked_blank' => '', 'locked_whitespace' => " \t\n", 'locked_unicode' => "\u00a0\u3000" }.fetch(sc['name'], jurisdiction)
        user.update_columns(wash_sale_enabled: true, wash_sale_jurisdiction: jurisdiction)
        user.wash_sale_locks.create!(asset_id: assets['AAA'], buy_locked_until: Figures.time(sc['at']) + 10.days, source: 'ledger')
      end
      bots.first.update_columns(redeploy_declined_offset: 100) if sc['name'] == 'stranded_offset'
      bots.first.bot_index_assets.delete_all if sc['name'] == 'no_composition'
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
        out['bots'] = bots.to_h do |bot|
          data = bot.metrics_with_current_prices
          marked = bot.metrics_with_current_prices_and_candles
          render = ->(partial, locals) { ApplicationController.render(partial:, locals:) }
          profit = bot.profit_in_usd(data, cache_only: false) unless user.hide_balances?
          [bot.id, {
            'tile' => render.call('bots/bot_tile/bot_tile_pnl',
                                  { bot:, pnl: data[:pnl], profit_usd: profit, denomination: user.denomination, loading: false }),
            'metrics' => render.call('bots/composition/metrics', { bot:, metrics: data, loading: false, exited_title_key: bot.exited_title_key }),
            'chart' => render.call('bots/chart', { bot:, metrics: marked, loading: false, current_user: user })
          }]
        end
        bots.each do |bot|
          bot.broadcast_pnl_update
          bot.broadcast_metrics_panel
          bot.broadcast_chart
        end
        user.broadcast_global_pnl_update
        snapshot = User::PnlHistory.snapshot(user, live: true)
        out['account'] = ApplicationController.render(partial: 'bots/global_pnl', locals: {
                                                        global_pnl: user.global_pnl(use_cache: false), history: snapshot[:result], loading: false,
                                                        hide_balances: user.hide_balances?, denomination: user.denomination
                                                      })
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
