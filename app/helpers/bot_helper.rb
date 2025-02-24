module BotHelper
  def bot_intervals
    [[t('bots.hour'), 'hour'], [t('bots.day'), 'day'], [t('bots.week'), 'week'], [t('bots.month'), 'month']]
  end

  def rounded_quote_amount_for(exchange:, base_asset:, quote_asset:, amount:)
    rounded_amount_for(exchange: exchange, base_asset: base_asset, quote_asset: quote_asset, amount: amount, amount_type: 'quote')
  end

  def rounded_base_amount_for(exchange:, base_asset:, quote_asset:, amount:)
    rounded_amount_for(exchange: exchange, base_asset: base_asset, quote_asset: quote_asset, amount: amount, amount_type: 'base')
  end

  def rounded_price_for(exchange:, base_asset:, quote_asset:, price:)
    return price if base_asset.nil? || quote_asset.nil?

    result = exchange.get_adjusted_price(
      base_asset: base_asset,
      quote_asset: quote_asset,
      price: price,
      method: :round
    )

    result.success? ? result.data : price
  end

  def rounded_chart_series_for(exchange:, base_asset:, quote_asset:, series:)
    return series if base_asset.nil? || quote_asset.nil?

    series.map do |serie|
      serie.map do |amount|
        rounded_quote_amount_for(exchange: exchange, base_asset: base_asset, quote_asset: quote_asset, amount: amount)
      end
    end
  end

  private

  def rounded_amount_for(exchange:, base_asset:, quote_asset:, amount:, amount_type:)
    return amount if base_asset.nil? || quote_asset.nil?

    result = exchange.get_adjusted_amount(
      base_asset: base_asset,
      quote_asset: quote_asset,
      amount: amount,
      amount_type: amount_type,
      method: :round
    )

    result.success? ? result.data : amount
  end
end
