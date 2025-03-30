module Bots::Barbell::Measurable
  extend ActiveSupport::Concern

  def fetch_metrics(force: false)
    metrics = Rails.cache.fetch(cache_key, expires_in: 30.days, force: force) do
      calculate_metrics
    end
    broadcast_metrics_update if force
    metrics
  end

  # TODO: add current price data
  #     add label + series
  #     update current_investment_value_in_quote
  #     update pnl

  private

  def cache_key
    "bot_#{id}_metrics"
  end

  def calculate_metrics
    data = initialize_metrics_data
    return data if true
    return data if transactions.empty?

    totals = initialize_totals_data

    transactions.order(created_at: :asc).each do |transaction|
      next if transaction.rate.zero? || transaction.base.nil?

      # chart data
      data[:chart][:labels] << transaction.created_at
      amount = transaction.success? ? transaction.amount : 0
      totals[:total_quote_amount_invested][transaction.base_asset.id] += amount * transaction.rate
      totals[:total_base_amount_acquired][transaction.base_asset.id] += amount
      data[:chart][:series][1] << totals[:total_quote_amount_invested].values.sum
      totals[:current_value_in_quote][transaction.base_asset.id] =
        totals[:total_base_amount_acquired][transaction.base_asset.id] * transaction.rate
      data[:chart][:series][0] << totals[:current_value_in_quote].values.sum

      # metrics data
      totals[:rates][transaction.base_asset.id] << transaction.rate
      totals[:amounts][transaction.base_asset.id] << amount
    end

    totals[:rates].each_with_index do |(base, rates_array), index|
      weighted_average = Utilities::Math.weighted_average(rates_array, totals[:amounts][base])
      data["base#{index}_average_buy_rate".to_sym] = weighted_average
    end

    data[:total_base0_amount_acquired] = totals[:total_base_amount_acquired][base0_asset_id]
    data[:total_base1_amount_acquired] = totals[:total_base_amount_acquired][base1_asset_id]
    from_quote_value = totals[:total_quote_amount_invested].values.sum
    to_quote_value = totals[:current_value_in_quote].values.sum
    data[:total_quote_amount_invested] = from_quote_value
    data[:current_investment_value_in_quote] = to_quote_value
    data[:pnl] = (to_quote_value - from_quote_value) / from_quote_value

    data
  end

  def initialize_metrics_data
    {
      chart: {
        labels: [],
        series: [
          [], # value
          []  # invested
        ]
      },
      total_base0_amount_acquired: 0,
      total_base1_amount_acquired: 0,
      total_quote_amount_invested: 0,
      current_investment_value_in_quote: 0,
      pnl: 0,
      base0_average_buy_rate: nil,
      base1_average_buy_rate: nil
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: { base0_asset_id => 0, base1_asset_id => 0 },
      total_base_amount_acquired: { base0_asset_id => 0, base1_asset_id => 0 },
      current_value_in_quote: { base0_asset_id => 0, base1_asset_id => 0 },
      rates: { base0_asset_id => [], base1_asset_id => [] },
      amounts: { base0_asset_id => [], base1_asset_id => [] }
    }
  end

  def broadcast_metrics_update
    broadcast_replace_to(
      ["bot_#{id}", :metrics],
      target: 'metrics',
      partial: 'bots/metrics/metrics',
      locals: { bot: self }
    )

    broadcast_replace_to(
      ["bot_#{id}", :chart],
      target: 'chart',
      partial: 'bots/chart',
      locals: { bot: self }
    )
  end
end
