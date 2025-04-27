module Bots::Barbell::Measurable
  extend ActiveSupport::Concern

  def metrics(force: false)
    metrics = Rails.cache.fetch("bot_#{id}_metrics", expires_in: 30.days, force: force) do
      calculate_metrics
    end
    broadcast_metrics_update if force
    metrics
  end

  def metrics_with_current_prices
    result = get_metrics_with_current_prices
    return result.data if result.success?

    metrics
  end

  def get_metrics_with_current_prices(force: false)
    metrics = metrics(force: force)
    return Result::Success.new(metrics) if metrics[:chart][:labels].empty?

    result0 = get_current_price(base_asset_id: base0_asset_id, quote_asset_id: quote_asset_id)
    return result0 unless result0.success?

    result1 = get_current_price(base_asset_id: base1_asset_id, quote_asset_id: quote_asset_id)
    return result1 unless result1.success?

    current_base0_value_in_quote = metrics[:total_base0_amount_acquired] * result0.data
    current_base1_value_in_quote = metrics[:total_base1_amount_acquired] * result1.data
    from_quote_value = metrics[:total_quote_amount_invested]
    to_quote_value = current_base0_value_in_quote + current_base1_value_in_quote
    metrics[:current_investment_value_in_quote] = to_quote_value
    metrics[:pnl] = (to_quote_value - from_quote_value) / from_quote_value
    metrics[:chart][:series][0] << to_quote_value
    metrics[:chart][:series][1] << from_quote_value
    metrics[:chart][:labels] << Time.current
    Result::Success.new(metrics)
  end

  private

  def get_current_price(base_asset_id:, quote_asset_id:)
    price = Rails.cache.fetch("exchange_#{exchange.id}_last_price_#{base_asset_id}_#{quote_asset_id}", expires_in: 5.minutes) do
      result = exchange.get_last_price(base_asset_id: base_asset_id, quote_asset_id: quote_asset_id)
      return result unless result.success?

      result.data
    end
    Result::Success.new(price)
  end

  def calculate_metrics
    data = initialize_metrics_data
    return data if transactions.empty? || transactions.where(status: :success).last.blank?

    totals = initialize_totals_data

    transactions.includes(:exchange).order(created_at: :asc).each do |transaction|
      next if transaction.rate.zero? || transaction.base.nil?

      # chart data
      base_asset_id = transaction.base_asset.id
      data[:chart][:labels] << transaction.created_at
      amount = transaction.success? ? transaction.amount : 0
      totals[:total_quote_amount_invested][base_asset_id] += amount * transaction.rate
      totals[:total_base_amount_acquired][base_asset_id] += amount
      data[:chart][:series][1] << totals[:total_quote_amount_invested].values.sum
      totals[:current_value_in_quote][base_asset_id] =
        totals[:total_base_amount_acquired][base_asset_id] * transaction.rate
      data[:chart][:series][0] << totals[:current_value_in_quote].values.sum

      # metrics data
      totals[:rates][base_asset_id] << transaction.rate
      totals[:amounts][base_asset_id] << amount
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
      pnl: nil,
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
