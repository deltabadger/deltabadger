module BotHelper
  def bot_intervals
    Bot::INTERVALS.map { |interval| [t("bot.#{interval}"), interval] }
  end

  def bot_type_label(bot)
    {
      'Bots::Barbell' => 'Barbell DCA',
      'Bots::Basic' => 'Basic DCA',
      'Bots::Withdrawal' => 'Withdrawal',
      'Bots::Webhook' => 'Webhook'
    }[bot.type]
  end

  def rounded_quote_amount_for(exchange:, base_asset_id:, quote_asset_id:, amount:)
    rounded_amount_for(
      exchange: exchange,
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: :quote
    )
  end

  def rounded_base_amount_for(exchange:, base_asset_id:, quote_asset_id:, amount:)
    rounded_amount_for(
      exchange: exchange,
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: :base
    )
  end

  def rounded_price_for(exchange:, base_asset_id:, quote_asset_id:, price:)
    return price if base_asset_id.nil? || quote_asset_id.nil? || price.nil?

    exchange.adjusted_price(
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      price: price,
      method: :round
    )
  end

  def rounded_chart_series_for(exchange:, base_asset_id:, quote_asset_id:, series:)
    return series if base_asset_id.nil? || quote_asset_id.nil?

    series.map do |serie|
      serie.map do |amount|
        rounded_quote_amount_for(
          exchange: exchange,
          base_asset_id: base_asset_id,
          quote_asset_id: quote_asset_id,
          amount: amount
        )
      end
    end
  end

  private

  # @param amount_type [Symbol] :base or :quote
  def rounded_amount_for(exchange:, base_asset_id:, quote_asset_id:, amount:, amount_type:)
    return amount if base_asset_id.nil? || quote_asset_id.nil? || amount.nil?

    exchange.adjusted_amount(
      base_asset_id: base_asset_id,
      quote_asset_id: quote_asset_id,
      amount: amount,
      amount_type: amount_type,
      method: :round
    )
  end
end
