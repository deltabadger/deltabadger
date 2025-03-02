module BarbellBot::Measurable
  extend ActiveSupport::Concern

  included do
    after_update_commit :broadcast_metrics_update, if: -> { saved_change_to_metrics_status? && metrics_ready? }
  end

  def metrics(recalculate: false)
    Rails.cache.delete("bot_#{id}_metrics") if recalculate
    data = Rails.cache.fetch("bot_#{id}_metrics", expires_in: 30.days) do
      update!(metrics_status: :pending)
      calculate_metrics
    end
    update!(metrics_status: :ready)
    data
  end

  private

  def calculate_metrics
    data = initialize_metrics_data
    totals = initialize_totals_data

    transactions.order(created_at: :asc).each do |transaction|
      next if transaction.rate.zero? || transaction.base.nil?

      # chart data
      data[:chart][:labels] << transaction.created_at
      amount = transaction.success? ? transaction.amount : 0
      totals[:total_quote_amount_invested][transaction.base] += amount * transaction.rate
      totals[:total_base_amount_acquired][transaction.base] += amount
      data[:chart][:series][1] << totals[:total_quote_amount_invested].values.sum
      totals[:current_value_in_quote][transaction.base] = totals[:total_base_amount_acquired][transaction.base] * transaction.rate
      data[:chart][:series][0] << totals[:current_value_in_quote].values.sum

      # metrics data
      totals[:rates][transaction.base] << transaction.rate
      totals[:amounts][transaction.base] << amount
    end

    totals[:rates].each_with_index do |(base, rates_array), index|
      weighted_average = Utilities::Math.weighted_average(rates_array, totals[:amounts][base])
      data["base#{index}_average_buy_rate".to_sym] = weighted_average
    end

    data[:total_base0_amount_acquired] = totals[:total_base_amount_acquired][base0]
    data[:total_base1_amount_acquired] = totals[:total_base_amount_acquired][base1]
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
      pnl: 0
    }
  end

  def initialize_totals_data
    {
      total_quote_amount_invested: { base0 => 0, base1 => 0 },
      total_base_amount_acquired: { base0 => 0, base1 => 0 },
      current_value_in_quote: { base0 => 0, base1 => 0 },
      rates: { base0 => [], base1 => [] },
      amounts: { base0 => [], base1 => [] }
    }
  end

  def broadcast_metrics_update
    broadcast_render_to(
      ["bot_#{id}", :metrics],
      partial: 'barbell_bots/metrics/metrics',
      locals: { bot: self }
    )
  end
end
