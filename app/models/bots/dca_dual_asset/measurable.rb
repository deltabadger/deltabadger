module Bots::DcaDualAsset::Measurable
  extend ActiveSupport::Concern

  # The fill arithmetic is shared with the index bot so the two can never disagree about what a
  # rebalance does to holdings, cost basis and realized cash.
  include Bot::RebalanceAccounting

  def metrics(force: false)
    # _v3: realised P/L and the contributed/ledger split. _v4: a per-leg cost-basis snapshot per
    # transaction, so the chart can draw one leg on its own. The old shape lives up to 30 days in the
    # cache, so the key must
    # change or every existing bot serves pre-rebalance numbers after deploy.
    cache_key = "bot_#{id}_metrics_v4"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(:created_at,
                                                                                :price,
                                                                                :amount_exec,
                                                                                :quote_amount_exec,
                                                                                :amount,
                                                                                :base,
                                                                                :side,
                                                                                :external_status,
                                                                                :transaction_type)
      return data if transactions_array.empty?

      # TODO: When transactions point to real asset ids, we can use the asset ids directly
      asset_symbol_to_id = {
        base0_asset.symbol => base0_asset_id,
        base1_asset.symbol => base1_asset_id
      }

      totals = initialize_totals_data
      ledger = totals[:ledger]
      books = totals[:books]
      transactions_array.each do |created_at, price, amount_exec, quote_amount_exec, amount, base, side, external_status, transaction_type|
        amount_exec, quote_amount_exec =
          confirmed_exec_amounts(external_status, price, amount, amount_exec, quote_amount_exec)
        next if price.blank? || quote_amount_exec.blank? || amount_exec.blank?
        next if quote_amount_exec.zero? || amount_exec.zero?

        asset_id = asset_symbol_to_id[base]
        next if asset_id.nil?

        data[:chart][:labels] << created_at

        branch = apply_fill(ledger, books, key: asset_id, side:, transaction_type:,
                                           amount_exec:, quote_amount_exec:)
        if branch == :regular_buy
          totals[:prices][asset_id] << price
          totals[:amounts][asset_id] << amount_exec
        end

        data[:chart][:series][1] << invested_total(books)
        totals[:current_value_in_quote][asset_id] = ledger[asset_id][:amount] * price
        data[:chart][:series][0] << portfolio_value(totals[:current_value_in_quote].values.sum, books)
        data[:chart][:extra_series][0] << ledger[base0_asset_id][:amount]
        data[:chart][:extra_series][1] << ledger[base1_asset_id][:amount]
        # What each leg cost, which is the zero line of its own curve when the user hovers its
        # row. NOT derivable from the total: a rebalance moves basis between the two legs.
        data[:chart][:invested_series][0] << ledger[base0_asset_id][:invested]
        data[:chart][:invested_series][1] << ledger[base1_asset_id][:invested]
        (data[:chart][:cash_series] ||= []) << uninvested_cash(books)
      end

      data[:total_base0_amount] = ledger[base0_asset_id][:amount]
      data[:total_base1_amount] = ledger[base1_asset_id][:amount]
      data[:base0_total_quote_amount_invested] = ledger[base0_asset_id][:invested]
      data[:base1_total_quote_amount_invested] = ledger[base1_asset_id][:invested]
      data[:total_quote_amount_invested] = invested_total(books)
      data[:total_base0_amount_value_in_quote] = totals[:current_value_in_quote][base0_asset_id]
      data[:total_base1_amount_value_in_quote] = totals[:current_value_in_quote][base1_asset_id]
      # Cash realized by a sell whose buy has not landed yet is still the user's money. Leaving it
      # out would show an artificial loss for the whole window between the two legs — and forever, if
      # the remainder ended as dust.
      data[:rebalance_cash] = uninvested_cash(books)
      data[:realised_pnl] = realised_pnl(books)
      data[:total_amount_value_in_quote] =
        data[:total_base0_amount_value_in_quote] + data[:total_base1_amount_value_in_quote] +
        data[:rebalance_cash]
      data[:base0_pnl] = calculate_pnl(data[:base0_total_quote_amount_invested], data[:total_base0_amount_value_in_quote])
      data[:base1_pnl] = calculate_pnl(data[:base1_total_quote_amount_invested], data[:total_base1_amount_value_in_quote])
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      if totals[:amounts][base0_asset_id].sum.positive?
        data[:base0_average_buy_price] =
          Utilities::Math.weighted_average(totals[:prices][base0_asset_id],
                                           totals[:amounts][base0_asset_id])
      end
      if totals[:amounts][base1_asset_id].sum.positive?
        data[:base1_average_buy_price] =
          Utilities::Math.weighted_average(totals[:prices][base1_asset_id],
                                           totals[:amounts][base1_asset_id])
      end

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics.deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty? || ticker0.nil? || ticker1.nil?

      result = exchange.get_tickers_prices(symbols: [ticker0.ticker, ticker1.ticker])
      return stale(metrics_data) if result.failure?

      price0 = result.data[ticker0.ticker]
      price1 = result.data[ticker1.ticker]
      return stale(metrics_data) unless price0.present? && price1.present?

      metrics_data[:total_base0_amount_value_in_quote] = metrics_data[:total_base0_amount] * price0
      metrics_data[:total_base1_amount_value_in_quote] = metrics_data[:total_base1_amount] * price1
      metrics_data[:total_amount_value_in_quote] =
        metrics_data[:total_base0_amount_value_in_quote] + metrics_data[:total_base1_amount_value_in_quote] +
        metrics_data[:rebalance_cash].to_d
      metrics_data[:base0_pnl] =
        calculate_pnl(metrics_data[:base0_total_quote_amount_invested], metrics_data[:total_base0_amount_value_in_quote])
      metrics_data[:base1_pnl] =
        calculate_pnl(metrics_data[:base1_total_quote_amount_invested], metrics_data[:total_base1_amount_value_in_quote])
      metrics_data[:pnl] = calculate_pnl(metrics_data[:total_quote_amount_invested], metrics_data[:total_amount_value_in_quote])
      metrics_data[:chart][:series][0] << metrics_data[:total_amount_value_in_quote]
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current
      # extra_series stays parallel with labels — the chart reads holdings by index.
      metrics_data[:chart][:extra_series][0] << metrics_data[:total_base0_amount]
      metrics_data[:chart][:extra_series][1] << metrics_data[:total_base1_amount]
      metrics_data[:chart][:invested_series] ||= [[], []]
      metrics_data[:chart][:invested_series][0] << metrics_data[:base0_total_quote_amount_invested]
      metrics_data[:chart][:invested_series][1] << metrics_data[:base1_total_quote_amount_invested]
      (metrics_data[:chart][:cash_series] ||= []) << metrics_data[:rebalance_cash].to_d
      # Raw per-symbol marks for the chart's price grid (see Bot::ChartSeries).
      metrics_data[:live_prices] = { ticker0.base => price0, ticker1.base => price1 }

      metrics_data
    end
  end

  def metrics_with_current_prices_and_candles(force: false)
    Rails.cache.fetch(metrics_with_current_prices_and_candles_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics_with_current_prices(force: force).deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty?

      grids = chart_price_grids(metrics_data)
      return metrics_data if grids.blank?

      metrics_data[:chart] = chart_marked_at_market(
        metrics_data[:chart], grids,
        holdings: lambda { |i|
          { ticker0.base => metrics_data[:chart][:extra_series][0][i],
            ticker1.base => metrics_data[:chart][:extra_series][1][i] }
        },
        basis: lambda { |i|
          basis = metrics_data[:chart][:invested_series] || [[], []]
          { ticker0.base => basis[0][i] || 0, ticker1.base => basis[1][i] || 0 }
        },
        # Cash a rebalance sell realized but its buy has not spent yet. Zero except between the two
        # legs — but marking those points at zero would draw a dip the portfolio never took.
        cash: ->(i) { (metrics_data[:chart][:cash_series] || [])[i] || 0 }
      )

      metrics_data
    end
  end

  def broadcast_metrics_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_dual_assets/metrics',
      locals: { bot: self, metrics: metrics_with_current_prices, loading: false }
    )

    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'chart',
      partial: 'bots/chart',
      locals: { bot: self, metrics: metrics_with_current_prices_and_candles, loading: false, current_user: user }
    )
  end

  def metrics_with_current_prices_from_cache
    Rails.cache.read(metrics_with_current_prices_cache_key)
  end

  def metrics_with_current_prices_and_candles_from_cache
    Rails.cache.read(metrics_with_current_prices_and_candles_cache_key)
  end

  private

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices_v4"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles_v4"
  end

  def calculate_pnl(from, to)
    return 0.0 if from.zero?

    (to - from).to_f / from
  end

  def initialize_metrics_data
    {
      chart: {
        labels: [],
        series: [
          [], # value
          []  # invested
        ],
        extra_series: [
          [], # base0 amount acquired
          []  # base1 amount acquired
        ],
        invested_series: [
          [], # base0 cost basis, for the per-leg curve
          []  # base1 cost basis
        ],
        cash_series: [] # rebalance proceeds realized but not yet redeployed
      },
      total_base0_amount: 0,
      total_base1_amount: 0,
      base0_total_quote_amount_invested: 0,
      base1_total_quote_amount_invested: 0,
      total_quote_amount_invested: 0,
      total_base0_amount_value_in_quote: 0,
      total_base1_amount_value_in_quote: 0,
      rebalance_cash: 0,
      realised_pnl: 0,
      total_amount_value_in_quote: 0,
      base0_pnl: nil,
      base1_pnl: nil,
      pnl: nil,
      base0_average_buy_price: nil,
      base1_average_buy_price: nil
    }
  end

  def initialize_totals_data
    {
      current_value_in_quote: { base0_asset_id => 0, base1_asset_id => 0 },
      prices: { base0_asset_id => [], base1_asset_id => [] },
      amounts: { base0_asset_id => [], base1_asset_id => [] },
      # Shared ledger shape (see Bot::RebalanceAccounting): holdings + cost basis per asset, plus
      # the off-ledger scalars — basis and cash in flight between a rebalance's two legs, lifetime
      # contributions, and realised liquidation cash and P/L.
      ledger: {
        base0_asset_id => { amount: 0, invested: 0 },
        base1_asset_id => { amount: 0, invested: 0 }
      },
      books: new_rebalance_books
    }
  end

  def optimal_candles_timeframe_for_duration(duration)
    # We want to show a chart of ~300 points when possible
    if duration < (1 * 300).minutes
      1.minute
    elsif duration < (5 * 300).minutes
      5.minutes
    elsif duration < (15 * 300).minutes
      15.minutes
    elsif duration < (30 * 300).minutes
      30.minutes
    elsif duration < (1 * 300).hours
      1.hour
    else
      1.day
    end
  end

  # The price grid both legs are marked on: each ticker's candle opens plus its live price.
  # Only the range where BOTH have marks can be re-marked — the every-symbol coverage rule in
  # Bot::ChartSeries takes care of that, so no explicit intersection is needed here.
  def chart_price_grids(metrics_data)
    return nil if ticker0.nil? || ticker1.nil?

    since, timeframe = chart_candle_window(metrics_data[:chart])
    grids = {}
    [ticker0, ticker1].each do |ticker|
      result = fetch_candle_series(ticker: ticker, since: since, timeframe: timeframe)
      return nil if result.failure? || result.data.blank?

      marks = result.data.map { |candle| [candle[0], candle[1]] } +
              chart_live_marks(metrics_data, ticker.base)
      grids[ticker.base] = marks.sort_by(&:first)
    end
    grids
  end
end
