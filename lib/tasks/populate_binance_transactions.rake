namespace :db do
  desc 'Populate transactions from Binance BTCUSDT 1H candles for bot 123'
  task populate_binance_transactions: :environment do
    binance = Exchange.find_by(type: 'Exchanges::Binance')
    ticker = binance.tickers.find_by(ticker: 'BTCUSDT')
    bot = Bot.find(123)
    start_at = Time.zone.now - 1.year
    timeframe = 1.hour

    result = binance.get_candles(ticker: ticker, start_at: start_at, timeframe: timeframe)

    if result.failure?
      puts "Error fetching candles: #{result.errors}"
      next
    end

    candles = result.data

    candles.each do |candle|
      time, open_price, = candle
      quote_amount_exec = 5.0
      amount_exec = (quote_amount_exec / open_price) * (1 - 0.001) # applying 0.1% penalty by reducing amount received

      Transaction.create!(
        bot_id: bot.id,
        exchange_id: binance.id,
        price: open_price,
        quote_amount: quote_amount_exec,
        quote_amount_exec: quote_amount_exec,
        amount: amount_exec,
        amount_exec: amount_exec,
        base: 'BTC',
        quote: 'USDT',
        side: :buy,
        order_type: :market_order,
        external_status: :closed,
        status: :submitted,
        created_at: time
      )
    end

    Bot::UpdateMetricsJob.perform_later(bot)
    puts "Populated #{candles.size} transactions and queued metrics update for bot 123"
  end
end
