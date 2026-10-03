# Loaded by pages.rb's figures command. Only harnesses change; Rails remains the oracle.
FIGURES_LIBRARY = true
require_relative 'figures'
module PageFigures
  module_function
  NAMES = %w[no_orders basket_buys row_readings smart_slices index_rotation liquidations tax_lots old_rows sold_out crypto_basket split price_untraded].freeze
  def run(root)
    Rails.logger.level = :warn
    Rails.cache = ActiveSupport::Cache::MemoryStore.new
    Faraday::Adapter.lookup_middleware(:net_http_persistent).prepend(ScriptedMarket::Adapter)
    template = Dir.mktmpdir('page-figures')
    assets = Figures.world(template)
    list = Figures.scenarios.select { |s| NAMES.include?(s['name']) }
    raise 'missing scenario' unless list.size == NAMES.size
    list << list.find { |s| s['name'] == 'basket_buys' }.merge('name' => 'hidden', 'hide_balances' => true)
    %w[locked locked_sold locked_gb locked_sold_gb stranded_offset no_composition].each do |name|
      source = name.start_with?('locked_sold') ? 'sold_out' : 'basket_buys'
      list << list.find { |s| s['name'] == source }.merge('name' => name)
    end
    list.each do |sc|
      dir = File.join(root, sc['name'])
      FileUtils.mkdir_p(dir)
      user, bots = Figures.build(sc.merge('dir' => dir), template, assets)
      user.update_columns(wash_sale_enabled: false)
      if sc['name'].start_with?('locked')
        jurisdiction = sc['name'].end_with?('_gb') ? 'GB' : 'US'
        user.update_columns(wash_sale_enabled: true, wash_sale_jurisdiction: jurisdiction)
        user.wash_sale_locks.create!(asset_id: assets['AAA'], buy_locked_until: Figures.time(sc['at']) + 10.days, source: 'ledger')
      end
      bots.first.update_columns(redeploy_declined_offset: 100) if sc['name'] == 'stranded_offset'
      bots.first.bot_index_assets.delete_all if sc['name'] == 'no_composition'
      Rails.cache.clear
      ScriptedMarket.http, ScriptedMarket.requests, ScriptedMarket.gaps = sc['script'], [], []
      File.write(File.join(dir, 'scenario.json'), JSON.pretty_generate({
        'parity_scratch' => true, 'at' => Figures.time(sc['at']).utc.iso8601(9), 'user_id' => user.id, 'bot_ids' => bots.map(&:id), 'script' => sc['script']
      }))
      before_offset = bots.first.reload.redeploy_declined_offset
      out = {}
      Figures.travel_to(Figures.time(sc['at']), with_usec: true) do
        out['bots'] = bots.to_h do |bot|
          data = bot.metrics_with_current_prices
          marked = bot.metrics_with_current_prices_and_candles
          render = ->(partial, locals) { ApplicationController.render(partial:, locals:) }
          [bot.id, {
            'tile' => render.call('bots/bot_tile/bot_tile_pnl', { bot:, pnl: data[:pnl], profit_usd: (bot.profit_in_usd(data, cache_only: false) unless user.hide_balances?), denomination: user.denomination, loading: false }),
            'metrics' => render.call('bots/composition/metrics', { bot:, metrics: data, loading: false, exited_title_key: bot.exited_title_key }),
            'chart' => render.call('bots/chart', { bot:, metrics: marked, loading: false, current_user: user })
          }]
        end
        snapshot = User::PnlHistory.snapshot(user, live: true)
        out['account'] = ApplicationController.render(partial: 'bots/global_pnl', locals: {
          global_pnl: user.global_pnl(use_cache: false), history: snapshot[:result], loading: false, hide_balances: user.hide_balances?, denomination: user.denomination
        })
      end
      raise 'unscripted market' unless ScriptedMarket.gaps.empty?
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
