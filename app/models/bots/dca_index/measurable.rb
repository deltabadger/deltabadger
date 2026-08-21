module Bots::DcaIndex::Measurable
  extend ActiveSupport::Concern

  # The fill arithmetic is shared with the dual-asset bot so the two can never disagree about what a
  # rebalance does to holdings, cost basis and realized cash.
  include Bot::RebalanceAccounting

  # Bounded so 100-asset bots don't stampede the exchange/proxy; tail fetches after
  # CandleSeriesCache are one small request each, so 6 workers keep latency flat.
  CANDLE_FETCH_THREADS = 6

  def metrics(force: false)
    # _v3: realised P/L and the contributed/ledger split. The old shape lives up to 30 days in the
    # cache, so the key must change or every existing bot serves pre-realisation numbers after deploy.
    cache_key = "bot_#{id}_metrics_v3"
    Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      data = initialize_metrics_data
      transactions_array = transactions.submitted.order(created_at: :asc).pluck(
        :created_at,
        :price,
        :amount_exec,
        :quote_amount_exec,
        :amount,
        :base,
        :side,
        :external_status,
        :transaction_type
      )
      return data if transactions_array.empty?

      totals = initialize_totals_data
      ledger = Hash.new { |hash, key| hash[key] = { amount: 0, invested: 0 } }
      books = new_rebalance_books
      asset_prices = {} # Track last known price for each asset

      transactions_array.each do |created_at, price, amount_exec, quote_amount_exec, amount, base, side, external_status, transaction_type|
        amount_exec, quote_amount_exec =
          confirmed_exec_amounts(external_status, price, amount, amount_exec, quote_amount_exec)
        next if price.blank? || quote_amount_exec.blank? || amount_exec.blank?
        next if quote_amount_exec.zero? || amount_exec.zero?

        branch = apply_fill(ledger, books, key: base, side:, transaction_type:,
                                           amount_exec:, quote_amount_exec:)
        asset_prices[base] = price

        # Chart data
        data[:chart][:labels] << created_at
        data[:chart][:series][0] << portfolio_value(ledger_value(ledger, asset_prices), books)
        data[:chart][:series][1] << invested_total(books)

        # Store snapshot of per-asset amounts for candle interpolation
        data[:chart][:extra_series] << ledger.transform_values { |entry| entry[:amount] }
        (data[:chart][:cash_series] ||= []) << uninvested_cash(books)

        # Metrics data — average entry price is over REGULAR buys only; a swap is not an entry.
        if branch == :regular_buy
          totals[:prices] << price
          totals[:amounts] << amount_exec
        end
      end

      data[:total_quote_amount_invested] = invested_total(books)
      # Cash realized by a sell whose buy has not landed yet — or by a liquidation the bot has not
      # re-spent — is still the user's money.
      data[:rebalance_cash] = uninvested_cash(books)
      data[:realised_pnl] = realised_pnl(books)
      data[:total_amount_value_in_quote] = portfolio_value(ledger_value(ledger, asset_prices), books)
      data[:pnl] = calculate_pnl(data[:total_quote_amount_invested], data[:total_amount_value_in_quote])
      # Public shape kept as-is (:quote_invested, not the ledger's :invested) — the order setter, the
      # live-price pass and the chart all read it. Plain hash: a default proc will not cache.
      data[:asset_breakdown] = ledger.each_with_object({}) do |(symbol, entry), acc|
        acc[symbol] = { amount: entry[:amount], quote_invested: entry[:invested] }
      end
      data[:num_assets] = ledger.count { |_symbol, entry| entry[:amount].positive? }

      data
    end
  end

  def metrics_with_current_prices(force: false)
    Rails.cache.fetch(metrics_with_current_prices_cache_key,
                      expires_in: Utilities::Time.seconds_to_end_of_five_minute_cut,
                      force: force) do
      metrics_data = metrics.deep_dup
      return metrics_data if metrics_data[:chart][:labels].empty?

      result = exchange.get_tickers_prices(symbols: tickers.map(&:ticker))
      return stale(metrics_data) if result.failure?

      ticker_prices = result.data
      total_value = 0

      # Raw per-symbol marks for the chart's price grid (see Bot::ChartSeries). Only symbols
      # that actually priced land here: a symbol missing from this hash has no upper endpoint,
      # so the chart leaves those points on their fill marks instead of marking a portfolio
      # that is silently missing an asset.
      live_prices = {}

      # Calculate current value for each asset
      asset_values = {}
      metrics_data[:asset_breakdown].each do |symbol, asset_data|
        # A fully liquidated holding keeps its ledger row (the chart reads holdings by index and the
        # series must stay parallel) but has nothing left to show. Without this an index bot that
        # rebalances accumulates a zero row per asset it has ever rotated out of.
        next unless asset_data[:amount].positive?

        ticker = tickers.find { |t| t.base == symbol }
        next unless ticker.present?

        price = ticker_prices[ticker.ticker]
        next unless price.present?

        live_prices[symbol] = price
        value = asset_data[:amount] * price
        total_value += value
        avg_price = asset_data[:amount].positive? ? asset_data[:quote_invested] / asset_data[:amount] : 0
        pnl_pct = asset_data[:quote_invested].positive? ? (value - asset_data[:quote_invested]) / asset_data[:quote_invested] : 0
        asset_values[symbol] = {
          amount: asset_data[:amount],
          quote_invested: asset_data[:quote_invested],
          current_value: value,
          current_price: price,
          avg_price: avg_price,
          pnl_percentage: pnl_pct
        }
      end

      total_value += metrics_data[:rebalance_cash].to_d
      metrics_data[:total_amount_value_in_quote] = total_value
      metrics_data[:pnl] = calculate_pnl(metrics_data[:total_quote_amount_invested], total_value)
      metrics_data[:asset_values] = asset_values
      metrics_data[:chart][:series][0] << total_value
      metrics_data[:chart][:series][1] << metrics_data[:total_quote_amount_invested]
      metrics_data[:chart][:labels] << Time.current
      # extra_series stays parallel with labels — the chart reads holdings by index.
      metrics_data[:chart][:extra_series] << metrics_data[:asset_breakdown].transform_values { |data| data[:amount] }
      (metrics_data[:chart][:cash_series] ||= []) << metrics_data[:rebalance_cash].to_d
      metrics_data[:live_prices] = live_prices

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

      # Only symbols the bot can price at all take part. An index rotates its composition, so it can
      # hold assets it no longer tracks (the user liquidates them by hand — see
      # Bots::DcaIndex::Liquidatable) — a DELISTED one has no ticker,
      # and `metrics_with_current_prices` already leaves them out of the live value. Demanding
      # candle coverage for them would leave almost every point uncovered (28 held symbols, 20
      # tickered, on a real bot) while the headline priced only the 20. A symbol that HAS a
      # ticker but no candles still blocks: there the chart would disagree with a headline that
      # prices it live.
      priceable = tickers.map(&:base)
      metrics_data[:chart] = chart_marked_at_market(
        metrics_data[:chart], grids,
        holdings: ->(i) { (metrics_data[:chart][:extra_series][i] || {}).slice(*priceable) },
        # Cash a rebalance sell realized but its buy has not spent yet — and, when a remainder ends
        # as dust, permanently. Marking those points at zero would draw a loss that never happened.
        cash: ->(i) { (metrics_data[:chart][:cash_series] || [])[i] || 0 }
      )

      metrics_data
    end
  end

  def broadcast_metrics_update
    broadcast_replace_to(
      ["user_#{user_id}", :bot_updates],
      target: 'metrics',
      partial: 'bots/dca_indexes/metrics',
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

  # Holdings valued at the last price each asset traded at. Used for the chart's running value and
  # for the fallback headline when the live read fails.
  def ledger_value(ledger, asset_prices)
    ledger.sum { |symbol, entry| entry[:amount] * (asset_prices[symbol] || 0) }
  end

  def metrics_with_current_prices_cache_key
    "bot_#{id}_metrics_with_current_prices_v3"
  end

  def metrics_with_current_prices_and_candles_cache_key
    "bot_#{id}_metrics_with_current_prices_and_candles_v3"
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

  # The price grid every held symbol is marked on: its candle opens plus its live price.
  # A symbol whose candles fail to fetch simply gets no grid, and Bot::ChartSeries then leaves
  # every point where that symbol is held on its fill mark — one bad ticker must not blank the
  # chart, and it must not silently price a held asset at zero either.
  def chart_price_grids(metrics_data)
    return nil if tickers.empty?

    symbols = metrics_data[:asset_breakdown].keys
    return nil if symbols.empty?

    since, timeframe = chart_candle_window(metrics_data[:chart])
    ticker_by_symbol = tickers.index_by(&:base)
    grids = {}

    # Bounded parallel batches. A raise in one worker becomes a per-symbol failure (logged);
    # failed symbols are skipped and retried on the next 5-minute metrics cycle.
    symbols.each_slice(CANDLE_FETCH_THREADS) do |batch|
      threads = batch.filter_map do |symbol|
        ticker = ticker_by_symbol[symbol]
        next unless ticker.present?

        Thread.new do
          result = begin
            Rails.application.executor.wrap do
              fetch_candle_series(ticker: ticker, since: since, timeframe: timeframe)
            end
          rescue StandardError => e
            Rails.logger.error("Candle fetch failed for #{symbol}: #{e.class}: #{e.message}")
            Result::Failure.new(e.message)
          end
          [symbol, result]
        end
      end

      ActiveSupport::Dependencies.interlock.permit_concurrent_loads do
        threads.each do |thread|
          symbol, result = thread.value
          next if result.failure? || result.data.blank?

          marks = result.data.map { |candle| [candle[0], candle[1]] } +
                  chart_live_marks(metrics_data, symbol)
          grids[symbol] = marks.sort_by(&:first)
        end
      end
    end

    grids
  end

  def fetch_candle_series(ticker:, since:, timeframe:)
    CandleSeriesCache.fetch(ticker: ticker, since: since, timeframe: timeframe)
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
        extra_series: [],
        cash_series: [] # rebalance proceeds realized but not yet redeployed
      },
      total_quote_amount_invested: 0,
      total_amount_value_in_quote: 0,
      rebalance_cash: 0,
      realised_pnl: 0,
      pnl: nil,
      asset_breakdown: {},
      asset_values: {},
      num_assets: 0
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: 0,
      prices: [],
      amounts: []
    }
  end
end
