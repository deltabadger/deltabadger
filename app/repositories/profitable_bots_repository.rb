class ProfitableBotsRepository
  def initialize
    @prices_dictionary = {}
    @markets_dictionary = {}
  end

  def profitable_bots_data
    bot_totals = ActiveRecord::Base.connection.execute('select * from bots_total_amounts')
    filtered_bots_map = bot_totals.to_a.map do |bot|
      profitable_or_nil(bot)
    end.compact
    bots_in_periods = [
      filtered_bots_map.filter { |x| x[1] <= Time.now - 12.month },
      filtered_bots_map.filter { |x| x[1] > Time.now - 12.month }
    ]
    total_amount_of_bots = bots_in_periods.map(&:length)
    total_amount_of_bots_in_profit = bots_in_periods.map { |bot| bot.count { |b| b[0] } }
    bots_in_periods.each_index do |index|
      percentage(total_amount_of_bots_in_profit[0..index].sum, total_amount_of_bots[0..index].sum)
    end
  end

  private

  def percentage(amount, total)
    (amount / total.to_f * 100).ceil(2)
  end

  def profitable_or_nil(bot)
    settings = JSON.parse(bot['settings'])
    current_price = current_price(bot['exchange_id'], settings['base'], settings['quote'])
    return nil if current_price.data.nil? || bot['total_amount'].nil? || !current_price.success?

    current_value = bot['total_amount'].to_f * current_price.data.to_f
    [current_value.to_f > bot['total_cost'].to_f, bot['created_at']]
  end

  def current_price(exchange_id, base, quote)
    dictionary_key = dictionary_key(base, quote)
    if @prices_dictionary[dictionary_key].nil?
      if @markets_dictionary[exchange_id].nil?
        market = ExchangeApi::Markets::Get.new.call(exchange_id)
        @markets_dictionary[exchange_id] = market
      else
        market = @markets_dictionary[exchange_id]
      end
      symbol = market.symbol(base, quote)
      current_price = market.current_price(symbol)
      @prices_dictionary[dictionary_key] = current_price
    else
      current_price = @prices_dictionary[dictionary_key]
    end
    current_price
  end

  def dictionary_key(base, quote)
    "#{base}-#{quote}"
  end
end
