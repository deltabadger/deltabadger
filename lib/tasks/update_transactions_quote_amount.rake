desc 'rake task to update transactions quote amount'
task update_transactions_quote_amount: :environment do
  exchanges = [
    Exchange.find_by(name: 'Coinbase'),
    Exchange.find_by(name: 'Kraken')
  ]
  puts 'getting bot ids'
  bot_ids = Transaction.where(exchange_id: exchanges.map(&:id), quote_amount: nil).where.not(external_id: nil).pluck(:bot_id).uniq
  puts 'getting user ids'
  user_ids = Bot.where(id: bot_ids).pluck(:user_id).uniq
  User.where(id: user_ids).find_each do |user|
    exchanges.each do |exchange|
      api_key = user.api_keys.find_by(exchange: exchange)
      next if api_key.blank?

      puts "setting client for #{exchange.name} for user #{user.id}"
      exchange.set_client(api_key: api_key)
      user_bots_ids = user.bots.barbell.where(exchange: exchange).pluck(:id).uniq
      puts "getting transactions for #{exchange.name} for user #{user.id}"
      Transaction.where(bot_id: user_bots_ids, exchange: exchange, quote_amount: nil)
                 .where.not(external_id: nil).find_each do |transaction|
        puts "getting order for #{transaction.external_id}"
        result = exchange.get_order(order_id: transaction.external_id)
        next unless result.success?

        puts "updating transaction #{transaction.id} with quote amount #{result.data[:quote_amount]}"
        transaction.update!(quote_amount: result.data[:quote_amount])
      end
    end
  end
end
